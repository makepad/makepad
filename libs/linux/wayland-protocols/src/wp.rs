//! Generated from protocol XML by `tools/wayland_codegen`.
//! Do not edit manually; regenerate instead.

pub mod tablet {
    pub mod zv2 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright 2014 © Stephen "Lyude" Chandler Paul
        // Copyright 2015-2024 © Red Hat, Inc.
        //
        // Permission is hereby granted, free of charge, to any person
        // obtaining a copy of this software and associated documentation files
        // (the "Software"), to deal in the Software without restriction,
        // including without limitation the rights to use, copy, modify, merge,
        // publish, distribute, sublicense, and/or sell copies of the Software,
        // and to permit persons to whom the Software is furnished to do so,
        // subject to the following conditions:
        //
        // The above copyright notice and this permission notice (including the
        // next paragraph) shall be included in all copies or substantial
        // portions of the Software.
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
        // EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
        // MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
        // NONINFRINGEMENT.  IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
        // BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
        // ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
        // CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
        // SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static ZWP_TABLET_MANAGER_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_manager_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "get_tablet_seat", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_TABLET_SEAT_V2_INTERFACE), arg_interfaces: &[&WL_SEAT_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&zwp_tablet_manager_v2_interface),
        };
        static zwp_tablet_manager_v2_requests_get_tablet_seat_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_seat_v2_interface,&wl_seat_interface,]);
        static zwp_tablet_manager_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_manager_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"get_tablet_seat".as_ptr(), signature: c"no".as_ptr(), types: zwp_tablet_manager_v2_requests_get_tablet_seat_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_manager_v2_requests_destroy_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_manager_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_manager_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_manager_v2_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static ZWP_TABLET_SEAT_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_seat_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "tablet_added", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_V2_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "tool_added", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_TOOL_V2_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "pad_added", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_PAD_V2_INTERFACE), arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_seat_v2_interface),
        };
        static zwp_tablet_seat_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_seat_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_seat_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_seat_v2_events_tablet_added_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_v2_interface,]);
        static zwp_tablet_seat_v2_events_tool_added_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_tool_v2_interface,]);
        static zwp_tablet_seat_v2_events_pad_added_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_pad_v2_interface,]);
        static zwp_tablet_seat_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"tablet_added".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_seat_v2_events_tablet_added_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"tool_added".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_seat_v2_events_tool_added_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"pad_added".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_seat_v2_events_pad_added_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_seat_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_seat_v2".as_ptr(), version: 2, request_count: 1, requests: zwp_tablet_seat_v2_requests.0.as_ptr(), event_count: 3, events: zwp_tablet_seat_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_TOOL_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_tool_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_cursor", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "type", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "hardware_serial", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "hardware_id_wacom", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "capability", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "removed", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "proximity_in", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&ZWP_TABLET_V2_INTERFACE,&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "proximity_out", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "down", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "up", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "motion", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "pressure", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "distance", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "tilt", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "rotation", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "slider", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "wheel", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "button", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "frame", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_tool_v2_interface),
        };
        static zwp_tablet_tool_v2_requests_set_cursor_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&wl_surface_interface,core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_tool_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_cursor".as_ptr(), signature: c"u?oii".as_ptr(), types: zwp_tablet_tool_v2_requests_set_cursor_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_tool_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_tool_v2_events_type_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_hardware_serial_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_hardware_id_wacom_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_capability_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_done_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_tool_v2_events_removed_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_tool_v2_events_proximity_in_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&zwp_tablet_v2_interface,&wl_surface_interface,]);
        static zwp_tablet_tool_v2_events_proximity_out_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_tool_v2_events_down_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_up_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_tool_v2_events_motion_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_pressure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_distance_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_tilt_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_rotation_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_slider_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_wheel_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_button_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_tool_v2_events_frame_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_tool_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"type".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_type_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"hardware_serial".as_ptr(), signature: c"uu".as_ptr(), types: zwp_tablet_tool_v2_events_hardware_serial_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"hardware_id_wacom".as_ptr(), signature: c"uu".as_ptr(), types: zwp_tablet_tool_v2_events_hardware_id_wacom_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"capability".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_capability_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_tool_v2_events_done_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"removed".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_tool_v2_events_removed_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"proximity_in".as_ptr(), signature: c"uoo".as_ptr(), types: zwp_tablet_tool_v2_events_proximity_in_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"proximity_out".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_tool_v2_events_proximity_out_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"down".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_down_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"up".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_tool_v2_events_up_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"motion".as_ptr(), signature: c"ff".as_ptr(), types: zwp_tablet_tool_v2_events_motion_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"pressure".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_pressure_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"distance".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_distance_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"tilt".as_ptr(), signature: c"ff".as_ptr(), types: zwp_tablet_tool_v2_events_tilt_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"rotation".as_ptr(), signature: c"f".as_ptr(), types: zwp_tablet_tool_v2_events_rotation_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"slider".as_ptr(), signature: c"i".as_ptr(), types: zwp_tablet_tool_v2_events_slider_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"wheel".as_ptr(), signature: c"fi".as_ptr(), types: zwp_tablet_tool_v2_events_wheel_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"button".as_ptr(), signature: c"uuu".as_ptr(), types: zwp_tablet_tool_v2_events_button_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"frame".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_tool_v2_events_frame_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_tool_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_tool_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_tool_v2_requests.0.as_ptr(), event_count: 19, events: zwp_tablet_tool_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "name", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "id", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "path", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "removed", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "bustype", since: 2, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_v2_interface),
        };
        static zwp_tablet_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_v2_events_name_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_v2_events_id_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_v2_events_path_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_v2_events_done_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_v2_events_removed_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_v2_events_bustype_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"name".as_ptr(), signature: c"s".as_ptr(), types: zwp_tablet_v2_events_name_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"id".as_ptr(), signature: c"uu".as_ptr(), types: zwp_tablet_v2_events_id_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"path".as_ptr(), signature: c"s".as_ptr(), types: zwp_tablet_v2_events_path_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_v2_events_done_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"removed".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_v2_events_removed_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"bustype".as_ptr(), signature: c"2u".as_ptr(), types: zwp_tablet_v2_events_bustype_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_v2".as_ptr(), version: 2, request_count: 1, requests: zwp_tablet_v2_requests.0.as_ptr(), event_count: 6, events: zwp_tablet_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_PAD_RING_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_pad_ring_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_feedback", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "source", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "angle", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "stop", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "frame", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_pad_ring_v2_interface),
        };
        static zwp_tablet_pad_ring_v2_requests_set_feedback_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_ring_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_ring_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_feedback".as_ptr(), signature: c"su".as_ptr(), types: zwp_tablet_pad_ring_v2_requests_set_feedback_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_ring_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_pad_ring_v2_events_source_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_ring_v2_events_angle_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_ring_v2_events_stop_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_ring_v2_events_frame_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_ring_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"source".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_ring_v2_events_source_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"angle".as_ptr(), signature: c"f".as_ptr(), types: zwp_tablet_pad_ring_v2_events_angle_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"stop".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_ring_v2_events_stop_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"frame".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_ring_v2_events_frame_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_pad_ring_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_pad_ring_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_pad_ring_v2_requests.0.as_ptr(), event_count: 4, events: zwp_tablet_pad_ring_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_PAD_STRIP_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_pad_strip_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_feedback", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "source", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "position", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "stop", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "frame", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_pad_strip_v2_interface),
        };
        static zwp_tablet_pad_strip_v2_requests_set_feedback_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_strip_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_strip_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_feedback".as_ptr(), signature: c"su".as_ptr(), types: zwp_tablet_pad_strip_v2_requests_set_feedback_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_strip_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_pad_strip_v2_events_source_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_strip_v2_events_position_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_strip_v2_events_stop_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_strip_v2_events_frame_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_strip_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"source".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_strip_v2_events_source_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"position".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_strip_v2_events_position_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"stop".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_strip_v2_events_stop_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"frame".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_strip_v2_events_frame_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_pad_strip_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_pad_strip_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_pad_strip_v2_requests.0.as_ptr(), event_count: 4, events: zwp_tablet_pad_strip_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_PAD_GROUP_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_pad_group_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "buttons", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Array,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "ring", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_PAD_RING_V2_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "strip", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_PAD_STRIP_V2_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "modes", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "mode_switch", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "dial", since: 2, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_PAD_DIAL_V2_INTERFACE), arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_pad_group_v2_interface),
        };
        static zwp_tablet_pad_group_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_group_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_group_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_pad_group_v2_events_buttons_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_group_v2_events_ring_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_pad_ring_v2_interface,]);
        static zwp_tablet_pad_group_v2_events_strip_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_pad_strip_v2_interface,]);
        static zwp_tablet_pad_group_v2_events_modes_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_group_v2_events_done_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_group_v2_events_mode_switch_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_group_v2_events_dial_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_pad_dial_v2_interface,]);
        static zwp_tablet_pad_group_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"buttons".as_ptr(), signature: c"a".as_ptr(), types: zwp_tablet_pad_group_v2_events_buttons_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"ring".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_pad_group_v2_events_ring_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"strip".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_pad_group_v2_events_strip_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"modes".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_group_v2_events_modes_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_group_v2_events_done_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"mode_switch".as_ptr(), signature: c"uuu".as_ptr(), types: zwp_tablet_pad_group_v2_events_mode_switch_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"dial".as_ptr(), signature: c"2n".as_ptr(), types: zwp_tablet_pad_group_v2_events_dial_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_pad_group_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_pad_group_v2".as_ptr(), version: 2, request_count: 1, requests: zwp_tablet_pad_group_v2_requests.0.as_ptr(), event_count: 7, events: zwp_tablet_pad_group_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_PAD_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_pad_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_feedback", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "group", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_TABLET_PAD_GROUP_V2_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "path", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "buttons", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "button", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "enter", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&ZWP_TABLET_V2_INTERFACE,&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "leave", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "removed", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_pad_v2_interface),
        };
        static zwp_tablet_pad_v2_requests_set_feedback_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_feedback".as_ptr(), signature: c"usu".as_ptr(), types: zwp_tablet_pad_v2_requests_set_feedback_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_pad_v2_events_group_types: InterfacePointers = InterfacePointers(&[&zwp_tablet_pad_group_v2_interface,]);
        static zwp_tablet_pad_v2_events_path_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_v2_events_buttons_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_v2_events_done_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_v2_events_button_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_v2_events_enter_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&zwp_tablet_v2_interface,&wl_surface_interface,]);
        static zwp_tablet_pad_v2_events_leave_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&wl_surface_interface,]);
        static zwp_tablet_pad_v2_events_removed_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"group".as_ptr(), signature: c"n".as_ptr(), types: zwp_tablet_pad_v2_events_group_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"path".as_ptr(), signature: c"s".as_ptr(), types: zwp_tablet_pad_v2_events_path_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"buttons".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_v2_events_buttons_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_v2_events_done_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"button".as_ptr(), signature: c"uuu".as_ptr(), types: zwp_tablet_pad_v2_events_button_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"enter".as_ptr(), signature: c"uoo".as_ptr(), types: zwp_tablet_pad_v2_events_enter_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"leave".as_ptr(), signature: c"uo".as_ptr(), types: zwp_tablet_pad_v2_events_leave_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"removed".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_v2_events_removed_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_pad_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_pad_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_pad_v2_requests.0.as_ptr(), event_count: 8, events: zwp_tablet_pad_v2_events.0.as_ptr(),
        };
        pub static ZWP_TABLET_PAD_DIAL_V2_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_tablet_pad_dial_v2", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_feedback", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "delta", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "frame", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_tablet_pad_dial_v2_interface),
        };
        static zwp_tablet_pad_dial_v2_requests_set_feedback_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_tablet_pad_dial_v2_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_tablet_pad_dial_v2_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_feedback".as_ptr(), signature: c"su".as_ptr(), types: zwp_tablet_pad_dial_v2_requests_set_feedback_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_tablet_pad_dial_v2_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_tablet_pad_dial_v2_events_delta_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_dial_v2_events_frame_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_tablet_pad_dial_v2_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"delta".as_ptr(), signature: c"i".as_ptr(), types: zwp_tablet_pad_dial_v2_events_delta_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"frame".as_ptr(), signature: c"u".as_ptr(), types: zwp_tablet_pad_dial_v2_events_frame_types.0.as_ptr() },
        ]);
        pub static zwp_tablet_pad_dial_v2_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_tablet_pad_dial_v2".as_ptr(), version: 2, request_count: 2, requests: zwp_tablet_pad_dial_v2_requests.0.as_ptr(), event_count: 2, events: zwp_tablet_pad_dial_v2_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "controller object for graphic tablet devices\n\nAn object that provides access to the graphics tablets available on this\nsystem. All tablets are associated with a seat, to get access to the\nactual tablets, use zwp_tablet_manager_v2.get_tablet_seat."]
        pub mod zwp_tablet_manager_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "get the tablet seat\n\nGet the zwp_tablet_seat_v2 object for the given seat. This object\nprovides access to all graphics tablets in this seat."]
            GetTabletSeat {
        #[doc = "The wl_seat object to retrieve the tablets for"]
                seat: super::wl_seat::WlSeat,
            },
        #[doc = "release the memory for the tablet manager object\n\nDestroy the zwp_tablet_manager_v2 object. Objects created from this\nobject are unaffected and should be destroyed separately."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::GetTabletSeat { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_GET_TABLET_SEAT_SINCE: u32 = 1;
        pub const REQ_GET_TABLET_SEAT_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct ZwpTabletManagerV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletManagerV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletManagerV2 {}
        impl PartialEq<Weak<ZwpTabletManagerV2>> for ZwpTabletManagerV2 {
            fn eq(&self, other: &Weak<ZwpTabletManagerV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletManagerV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletManagerV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletManagerV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_MANAGER_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::GetTabletSeat { seat } => {
                    let child_spec = Some((super::zwp_tablet_seat_v2::ZwpTabletSeatV2::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(seat.id()));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletManagerV2 {
        #[doc = "get the tablet seat\n\nGet the zwp_tablet_seat_v2 object for the given seat. This object\nprovides access to all graphics tablets in this seat."]
        pub fn get_tablet_seat<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_tablet_seat_v2::ZwpTabletSeatV2, U> + 'static>(&self, seat: &super::wl_seat::WlSeat, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_tablet_seat_v2::ZwpTabletSeatV2 {
            self.send_constructor::<super::zwp_tablet_seat_v2::ZwpTabletSeatV2>(Request::GetTabletSeat { seat: seat.clone() }, qh.make_data::<super::zwp_tablet_seat_v2::ZwpTabletSeatV2, U>(udata)).unwrap_or_else(|_| super::zwp_tablet_seat_v2::ZwpTabletSeatV2::inert(self.backend.clone()))
        }
        #[doc = "release the memory for the tablet manager object\n\nDestroy the zwp_tablet_manager_v2 object. Objects created from this\nobject are unaffected and should be destroyed separately."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "controller object for graphic tablet devices of a seat\n\nAn object that provides access to the graphics tablets available on this\nseat. After binding to this interface, the compositor sends a set of\nzwp_tablet_seat_v2.tablet_added and zwp_tablet_seat_v2.tool_added events."]
        pub mod zwp_tablet_seat_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "release the memory for the tablet seat object\n\nDestroy the zwp_tablet_seat_v2 object. Objects created from this\nobject are unaffected and should be destroyed separately."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "new device notification\n\nThis event is sent whenever a new tablet becomes available on this\nseat. This event only provides the object id of the tablet, any\nstatic information about the tablet (device name, vid/pid, etc.) is\nsent through the zwp_tablet_v2 interface."]
            TabletAdded {
        #[doc = "the newly added graphics tablet"]
                id: super::zwp_tablet_v2::ZwpTabletV2,
            },
        #[doc = "a new tool has been used with a tablet\n\nThis event is sent whenever a tool that has not previously been used\nwith a tablet comes into use. This event only provides the object id\nof the tool; any static information about the tool (capabilities,\ntype, etc.) is sent through the zwp_tablet_tool_v2 interface."]
            ToolAdded {
        #[doc = "the newly added tablet tool"]
                id: super::zwp_tablet_tool_v2::ZwpTabletToolV2,
            },
        #[doc = "new pad notification\n\nThis event is sent whenever a new pad is known to the system. Typically,\npads are physically attached to tablets and a pad_added event is\nsent immediately after the zwp_tablet_seat_v2.tablet_added.\nHowever, some standalone pad devices logically attach to tablets at\nruntime, and the client must wait for zwp_tablet_pad_v2.enter to know\nthe tablet a pad is attached to.\n\nThis event only provides the object id of the pad. All further\nfeatures (buttons, strips, rings) are sent through the zwp_tablet_pad_v2\ninterface."]
            PadAdded {
        #[doc = "the newly added pad"]
                id: super::zwp_tablet_pad_v2::ZwpTabletPadV2,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::TabletAdded { .. } => 0,
            Self::ToolAdded { .. } => 1,
            Self::PadAdded { .. } => 2,
        } } }
        pub const EVT_TABLET_ADDED_SINCE: u32 = 1;
        pub const EVT_TABLET_ADDED_OPCODE: u16 = 0;
        pub const EVT_TOOL_ADDED_SINCE: u32 = 1;
        pub const EVT_TOOL_ADDED_OPCODE: u16 = 1;
        pub const EVT_PAD_ADDED_SINCE: u32 = 1;
        pub const EVT_PAD_ADDED_OPCODE: u16 = 2;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletSeatV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletSeatV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletSeatV2 {}
        impl PartialEq<Weak<ZwpTabletSeatV2>> for ZwpTabletSeatV2 {
            fn eq(&self, other: &Weak<ZwpTabletSeatV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletSeatV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletSeatV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletSeatV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_SEAT_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::NewId(id)),) = (arg_iter.next(),) {
                        Ok((me, Event::TabletAdded { id: super::zwp_tablet_v2::ZwpTabletV2::from_id(conn, id.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::NewId(id)),) = (arg_iter.next(),) {
                        Ok((me, Event::ToolAdded { id: super::zwp_tablet_tool_v2::ZwpTabletToolV2::from_id(conn, id.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::NewId(id)),) = (arg_iter.next(),) {
                        Ok((me, Event::PadAdded { id: super::zwp_tablet_pad_v2::ZwpTabletPadV2::from_id(conn, id.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletSeatV2 {
        #[doc = "release the memory for the tablet seat object\n\nDestroy the zwp_tablet_seat_v2 object. Objects created from this\nobject are unaffected and should be destroyed separately."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "a physical tablet tool\n\nAn object that represents a physical tool that has been, or is\ncurrently in use with a tablet in this seat. Each zwp_tablet_tool_v2\nobject stays valid until the client destroys it; the compositor\nreuses the zwp_tablet_tool_v2 object to indicate that the object's\nrespective physical tool has come into proximity of a tablet again.\n\nA zwp_tablet_tool_v2 object's relation to a physical tool depends on the\ntablet's ability to report serial numbers. If the tablet supports\nthis capability, then the object represents a specific physical tool\nand can be identified even when used on multiple tablets.\n\nA tablet tool has a number of static characteristics, e.g. tool type,\nhardware_serial and capabilities. These capabilities are sent in an\nevent sequence after the zwp_tablet_seat_v2.tool_added event before any\nactual events from this tool. This initial event sequence is\nterminated by a zwp_tablet_tool_v2.done event.\n\nTablet tool events are grouped by zwp_tablet_tool_v2.frame events.\nAny events received before a zwp_tablet_tool_v2.frame event should be\nconsidered part of the same hardware state change."]
        pub mod zwp_tablet_tool_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "a physical tool type\n\nDescribes the physical type of a tool. The physical type of a tool\ngenerally defines its base usage.\n\nThe mouse tool represents a mouse-shaped tool that is not a relative\ndevice but bound to the tablet's surface, providing absolute\ncoordinates.\n\nThe lens tool is a mouse-shaped tool with an attached lens to\nprovide precision focus."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Type {
        #[doc = "Pen"]
            Pen = 320,
        #[doc = "Eraser"]
            Eraser = 321,
        #[doc = "Brush"]
            Brush = 322,
        #[doc = "Pencil"]
            Pencil = 323,
        #[doc = "Airbrush"]
            Airbrush = 324,
        #[doc = "Finger"]
            Finger = 325,
        #[doc = "Mouse"]
            Mouse = 326,
        #[doc = "Lens"]
            Lens = 327,
        }
        impl TryFrom<u32> for Type {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                320 => Ok(Self::Pen),
                321 => Ok(Self::Eraser),
                322 => Ok(Self::Brush),
                323 => Ok(Self::Pencil),
                324 => Ok(Self::Airbrush),
                325 => Ok(Self::Finger),
                326 => Ok(Self::Mouse),
                327 => Ok(Self::Lens),
                _ => Err(()),
            } }
        }
        impl From<Type> for u32 { fn from(value: Type) -> u32 { value as u32 } }
        #[doc = "capability flags for a tool\n\nDescribes extra capabilities on a tablet.\n\nAny tool must provide x and y values, extra axes are\ndevice-specific."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Capability {
        #[doc = "Tilt axes"]
            Tilt = 1,
        #[doc = "Pressure axis"]
            Pressure = 2,
        #[doc = "Distance axis"]
            Distance = 3,
        #[doc = "Z-rotation axis"]
            Rotation = 4,
        #[doc = "Slider axis"]
            Slider = 5,
        #[doc = "Wheel axis"]
            Wheel = 6,
        }
        impl TryFrom<u32> for Capability {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::Tilt),
                2 => Ok(Self::Pressure),
                3 => Ok(Self::Distance),
                4 => Ok(Self::Rotation),
                5 => Ok(Self::Slider),
                6 => Ok(Self::Wheel),
                _ => Err(()),
            } }
        }
        impl From<Capability> for u32 { fn from(value: Capability) -> u32 { value as u32 } }
        #[doc = "physical button state\n\nDescribes the physical state of a button that produced the button event."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum ButtonState {
        #[doc = "button is not pressed"]
            Released = 0,
        #[doc = "button is pressed"]
            Pressed = 1,
        }
        impl TryFrom<u32> for ButtonState {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::Released),
                1 => Ok(Self::Pressed),
                _ => Err(()),
            } }
        }
        impl From<ButtonState> for u32 { fn from(value: ButtonState) -> u32 { value as u32 } }
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Error {
        #[doc = "given wl_surface has another role"]
            Role = 0,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::Role),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set the tablet tool's surface\n\nSets the surface of the cursor used for this tool on the given\ntablet. This request only takes effect if the tool is in proximity\nof one of the requesting client's surfaces or the surface parameter\nis the current pointer surface. If there was a previous surface set\nwith this request it is replaced. If surface is NULL, the cursor\nimage is hidden.\n\nThe parameters hotspot_x and hotspot_y define the position of the\npointer surface relative to the pointer location. Its top-left corner\nis always at (x, y) - (hotspot_x, hotspot_y), where (x, y) are the\ncoordinates of the pointer location, in surface-local coordinates.\n\nOn surface.attach requests to the pointer surface, hotspot_x and\nhotspot_y are decremented by the x and y parameters passed to the\nrequest. Attach must be confirmed by wl_surface.commit as usual.\n\nThe hotspot can also be updated by passing the currently set pointer\nsurface to this request with new values for hotspot_x and hotspot_y.\n\nThe current and pending input regions of the wl_surface are cleared,\nand wl_surface.set_input_region is ignored until the wl_surface is no\nlonger used as the cursor. When the use as a cursor ends, the current\nand pending input regions become undefined, and the wl_surface is\nunmapped.\n\nThis request gives the surface the role of a zwp_tablet_tool_v2 cursor. A\nsurface may only ever be used as the cursor surface for one\nzwp_tablet_tool_v2. If the surface already has another role or has\npreviously been used as cursor surface for a different tool, a\nprotocol error is raised."]
            SetCursor {
        #[doc = "serial of the proximity_in event"]
                serial: u32,
                surface: Option<super::wl_surface::WlSurface>,
        #[doc = "surface-local x coordinate"]
                hotspot_x: i32,
        #[doc = "surface-local y coordinate"]
                hotspot_y: i32,
            },
        #[doc = "destroy the tool object\n\nThis destroys the client's resource for this tool object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetCursor { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_CURSOR_SINCE: u32 = 1;
        pub const REQ_SET_CURSOR_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "tool type\n\nThe tool type is the high-level type of the tool and usually decides\nthe interaction expected from this tool.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_tool_v2.done event."]
            Type {
        #[doc = "the physical tool type"]
                tool_type: WEnum<Type>,
            },
        #[doc = "unique hardware serial number of the tool\n\nIf the physical tool can be identified by a unique 64-bit serial\nnumber, this event notifies the client of this serial number.\n\nIf multiple tablets are available in the same seat and the tool is\nuniquely identifiable by the serial number, that tool may move\nbetween tablets.\n\nOtherwise, if the tool has no serial number and this event is\nmissing, the tool is tied to the tablet it first comes into\nproximity with. Even if the physical tool is used on multiple\ntablets, separate zwp_tablet_tool_v2 objects will be created, one per\ntablet.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_tool_v2.done event."]
            HardwareSerial {
        #[doc = "the unique serial number of the tool, most significant bits"]
                hardware_serial_hi: u32,
        #[doc = "the unique serial number of the tool, least significant bits"]
                hardware_serial_lo: u32,
            },
        #[doc = "hardware id notification in Wacom's format\n\nThis event notifies the client of a hardware id available on this tool.\n\nThe hardware id is a device-specific 64-bit id that provides extra\ninformation about the tool in use, beyond the wl_tool.type\nenumeration. The format of the id is specific to tablets made by\nWacom Inc. For example, the hardware id of a Wacom Grip\nPen (a stylus) is 0x802.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_tool_v2.done event."]
            HardwareIdWacom {
        #[doc = "the hardware id, most significant bits"]
                hardware_id_hi: u32,
        #[doc = "the hardware id, least significant bits"]
                hardware_id_lo: u32,
            },
        #[doc = "tool capability notification\n\nThis event notifies the client of any capabilities of this tool,\nbeyond the main set of x/y axes and tip up/down detection.\n\nOne event is sent for each extra capability available on this tool.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_tool_v2.done event."]
            Capability {
        #[doc = "the capability"]
                capability: WEnum<Capability>,
            },
        #[doc = "tool description events sequence complete\n\nThis event signals the end of the initial burst of descriptive\nevents. A client may consider the static description of the tool to\nbe complete and finalize initialization of the tool."]
            Done,
        #[doc = "tool removed\n\nThis event is sent when the tool is removed from the system and will\nsend no further events. Should the physical tool come back into\nproximity later, a new zwp_tablet_tool_v2 object will be created.\n\nIt is compositor-dependent when a tool is removed. A compositor may\nremove a tool on proximity out, tablet removal or any other reason.\nA compositor may also keep a tool alive until shutdown.\n\nIf the tool is currently in proximity, a proximity_out event will be\nsent before the removed event. See zwp_tablet_tool_v2.proximity_out for\nthe handling of any buttons logically down.\n\nWhen this event is received, the client must zwp_tablet_tool_v2.destroy\nthe object."]
            Removed,
        #[doc = "proximity in event\n\nNotification that this tool is focused on a certain surface.\n\nThis event can be received when the tool has moved from one surface to\nanother, or when the tool has come back into proximity above the\nsurface.\n\nIf any button is logically down when the tool comes into proximity,\nthe respective button event is sent after the proximity_in event but\nwithin the same frame as the proximity_in event."]
            ProximityIn {
                serial: u32,
        #[doc = "The tablet the tool is in proximity of"]
                tablet: super::zwp_tablet_v2::ZwpTabletV2,
        #[doc = "The current surface the tablet tool is over"]
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "proximity out event\n\nNotification that this tool has either left proximity, or is no\nlonger focused on a certain surface.\n\nWhen the tablet tool leaves proximity of the tablet, button release\nevents are sent for each button that was held down at the time of\nleaving proximity. These events are sent before the proximity_out\nevent but within the same zwp_tablet_v2.frame.\n\nIf the tool stays within proximity of the tablet, but the focus\nchanges from one surface to another, a button release event may not\nbe sent until the button is actually released or the tool leaves the\nproximity of the tablet."]
            ProximityOut,
        #[doc = "tablet tool is making contact\n\nSent whenever the tablet tool comes in contact with the surface of the\ntablet.\n\nIf the tool is already in contact with the tablet when entering the\ninput region, the client owning said region will receive a\nzwp_tablet_v2.proximity_in event, followed by a zwp_tablet_v2.down\nevent and a zwp_tablet_v2.frame event.\n\nNote that this event describes logical contact, not physical\ncontact. On some devices, a compositor may not consider a tool in\nlogical contact until a minimum physical pressure threshold is\nexceeded."]
            Down {
                serial: u32,
            },
        #[doc = "tablet tool is no longer making contact\n\nSent whenever the tablet tool stops making contact with the surface of\nthe tablet, or when the tablet tool moves out of the input region\nand the compositor grab (if any) is dismissed.\n\nIf the tablet tool moves out of the input region while in contact\nwith the surface of the tablet and the compositor does not have an\nongoing grab on the surface, the client owning said region will\nreceive a zwp_tablet_v2.up event, followed by a zwp_tablet_v2.proximity_out\nevent and a zwp_tablet_v2.frame event. If the compositor has an ongoing\ngrab on this device, this event sequence is sent whenever the grab\nis dismissed in the future.\n\nNote that this event describes logical contact, not physical\ncontact. On some devices, a compositor may not consider a tool out\nof logical contact until physical pressure falls below a specific\nthreshold."]
            Up,
        #[doc = "motion event\n\nSent whenever a tablet tool moves."]
            Motion {
        #[doc = "surface-local x coordinate"]
                x: f64,
        #[doc = "surface-local y coordinate"]
                y: f64,
            },
        #[doc = "pressure change event\n\nSent whenever the pressure axis on a tool changes. The value of this\nevent is normalized to a value between 0 and 65535.\n\nNote that pressure may be nonzero even when a tool is not in logical\ncontact. See the down and up events for more details."]
            Pressure {
        #[doc = "The current pressure value"]
                pressure: u32,
            },
        #[doc = "distance change event\n\nSent whenever the distance axis on a tool changes. The value of this\nevent is normalized to a value between 0 and 65535.\n\nNote that distance may be nonzero even when a tool is not in logical\ncontact. See the down and up events for more details."]
            Distance {
        #[doc = "The current distance value"]
                distance: u32,
            },
        #[doc = "tilt change event\n\nSent whenever one or both of the tilt axes on a tool change. Each tilt\nvalue is in degrees, relative to the z-axis of the tablet.\nThe angle is positive when the top of a tool tilts along the\npositive x or y axis."]
            Tilt {
        #[doc = "The current value of the X tilt axis"]
                tilt_x: f64,
        #[doc = "The current value of the Y tilt axis"]
                tilt_y: f64,
            },
        #[doc = "z-rotation change event\n\nSent whenever the z-rotation axis on the tool changes. The\nrotation value is in degrees clockwise from the tool's\nlogical neutral position."]
            Rotation {
        #[doc = "The current rotation of the Z axis"]
                degrees: f64,
            },
        #[doc = "Slider position change event\n\nSent whenever the slider position on the tool changes. The\nvalue is normalized between -65535 and 65535, with 0 as the logical\nneutral position of the slider.\n\nThe slider is available on e.g. the Wacom Airbrush tool."]
            Slider {
        #[doc = "The current position of slider"]
                position: i32,
            },
        #[doc = "Wheel delta event\n\nSent whenever the wheel on the tool emits an event. This event\ncontains two values for the same axis change. The degrees value is\nin the same orientation as the wl_pointer.vertical_scroll axis. The\nclicks value is in discrete logical clicks of the mouse wheel. This\nvalue may be zero if the movement of the wheel was less\nthan one logical click.\n\nClients should choose either value and avoid mixing degrees and\nclicks. The compositor may accumulate values smaller than a logical\nclick and emulate click events when a certain threshold is met.\nThus, zwp_tablet_tool_v2.wheel events with non-zero clicks values may\nhave different degrees values."]
            Wheel {
        #[doc = "The wheel delta in degrees"]
                degrees: f64,
        #[doc = "The wheel delta in discrete clicks"]
                clicks: i32,
            },
        #[doc = "button event\n\nSent whenever a button on the tool is pressed or released.\n\nIf a button is held down when the tool moves in or out of proximity,\nbutton events are generated by the compositor. See\nzwp_tablet_tool_v2.proximity_in and zwp_tablet_tool_v2.proximity_out for\ndetails."]
            Button {
                serial: u32,
        #[doc = "The button whose state has changed"]
                button: u32,
        #[doc = "Whether the button was pressed or released"]
                state: WEnum<ButtonState>,
            },
        #[doc = "frame event\n\nMarks the end of a series of axis and/or button updates from the\ntablet. The Wayland protocol requires axis updates to be sent\nsequentially, however all events within a frame should be considered\none hardware event."]
            Frame {
        #[doc = "The time of the event with millisecond granularity"]
                time: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Type { .. } => 0,
            Self::HardwareSerial { .. } => 1,
            Self::HardwareIdWacom { .. } => 2,
            Self::Capability { .. } => 3,
            Self::Done => 4,
            Self::Removed => 5,
            Self::ProximityIn { .. } => 6,
            Self::ProximityOut => 7,
            Self::Down { .. } => 8,
            Self::Up => 9,
            Self::Motion { .. } => 10,
            Self::Pressure { .. } => 11,
            Self::Distance { .. } => 12,
            Self::Tilt { .. } => 13,
            Self::Rotation { .. } => 14,
            Self::Slider { .. } => 15,
            Self::Wheel { .. } => 16,
            Self::Button { .. } => 17,
            Self::Frame { .. } => 18,
        } } }
        pub const EVT_TYPE_SINCE: u32 = 1;
        pub const EVT_TYPE_OPCODE: u16 = 0;
        pub const EVT_HARDWARE_SERIAL_SINCE: u32 = 1;
        pub const EVT_HARDWARE_SERIAL_OPCODE: u16 = 1;
        pub const EVT_HARDWARE_ID_WACOM_SINCE: u32 = 1;
        pub const EVT_HARDWARE_ID_WACOM_OPCODE: u16 = 2;
        pub const EVT_CAPABILITY_SINCE: u32 = 1;
        pub const EVT_CAPABILITY_OPCODE: u16 = 3;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 4;
        pub const EVT_REMOVED_SINCE: u32 = 1;
        pub const EVT_REMOVED_OPCODE: u16 = 5;
        pub const EVT_PROXIMITY_IN_SINCE: u32 = 1;
        pub const EVT_PROXIMITY_IN_OPCODE: u16 = 6;
        pub const EVT_PROXIMITY_OUT_SINCE: u32 = 1;
        pub const EVT_PROXIMITY_OUT_OPCODE: u16 = 7;
        pub const EVT_DOWN_SINCE: u32 = 1;
        pub const EVT_DOWN_OPCODE: u16 = 8;
        pub const EVT_UP_SINCE: u32 = 1;
        pub const EVT_UP_OPCODE: u16 = 9;
        pub const EVT_MOTION_SINCE: u32 = 1;
        pub const EVT_MOTION_OPCODE: u16 = 10;
        pub const EVT_PRESSURE_SINCE: u32 = 1;
        pub const EVT_PRESSURE_OPCODE: u16 = 11;
        pub const EVT_DISTANCE_SINCE: u32 = 1;
        pub const EVT_DISTANCE_OPCODE: u16 = 12;
        pub const EVT_TILT_SINCE: u32 = 1;
        pub const EVT_TILT_OPCODE: u16 = 13;
        pub const EVT_ROTATION_SINCE: u32 = 1;
        pub const EVT_ROTATION_OPCODE: u16 = 14;
        pub const EVT_SLIDER_SINCE: u32 = 1;
        pub const EVT_SLIDER_OPCODE: u16 = 15;
        pub const EVT_WHEEL_SINCE: u32 = 1;
        pub const EVT_WHEEL_OPCODE: u16 = 16;
        pub const EVT_BUTTON_SINCE: u32 = 1;
        pub const EVT_BUTTON_OPCODE: u16 = 17;
        pub const EVT_FRAME_SINCE: u32 = 1;
        pub const EVT_FRAME_OPCODE: u16 = 18;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletToolV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletToolV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletToolV2 {}
        impl PartialEq<Weak<ZwpTabletToolV2>> for ZwpTabletToolV2 {
            fn eq(&self, other: &Weak<ZwpTabletToolV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletToolV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletToolV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletToolV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_TOOL_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(tool_type)),) = (arg_iter.next(),) {
                        Ok((me, Event::Type { tool_type: WEnum::from(tool_type as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(hardware_serial_hi)), Some(Argument::Uint(hardware_serial_lo)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::HardwareSerial { hardware_serial_hi: hardware_serial_hi, hardware_serial_lo: hardware_serial_lo }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Uint(hardware_id_hi)), Some(Argument::Uint(hardware_id_lo)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::HardwareIdWacom { hardware_id_hi: hardware_id_hi, hardware_id_lo: hardware_id_lo }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                3 => {
                    if let (Some(Argument::Uint(capability)),) = (arg_iter.next(),) {
                        Ok((me, Event::Capability { capability: WEnum::from(capability as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                4 => Ok((me, Event::Done)),
                5 => Ok((me, Event::Removed)),
                6 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Object(tablet)), Some(Argument::Object(surface)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::ProximityIn { serial: serial, tablet: super::zwp_tablet_v2::ZwpTabletV2::from_id(conn, tablet.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                7 => Ok((me, Event::ProximityOut)),
                8 => {
                    if let (Some(Argument::Uint(serial)),) = (arg_iter.next(),) {
                        Ok((me, Event::Down { serial: serial }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                9 => Ok((me, Event::Up)),
                10 => {
                    if let (Some(Argument::Fixed(x)), Some(Argument::Fixed(y)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Motion { x: (x as f64) / 256.0, y: (y as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                11 => {
                    if let (Some(Argument::Uint(pressure)),) = (arg_iter.next(),) {
                        Ok((me, Event::Pressure { pressure: pressure }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                12 => {
                    if let (Some(Argument::Uint(distance)),) = (arg_iter.next(),) {
                        Ok((me, Event::Distance { distance: distance }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                13 => {
                    if let (Some(Argument::Fixed(tilt_x)), Some(Argument::Fixed(tilt_y)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Tilt { tilt_x: (tilt_x as f64) / 256.0, tilt_y: (tilt_y as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                14 => {
                    if let (Some(Argument::Fixed(degrees)),) = (arg_iter.next(),) {
                        Ok((me, Event::Rotation { degrees: (degrees as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                15 => {
                    if let (Some(Argument::Int(position)),) = (arg_iter.next(),) {
                        Ok((me, Event::Slider { position: position }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                16 => {
                    if let (Some(Argument::Fixed(degrees)), Some(Argument::Int(clicks)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Wheel { degrees: (degrees as f64) / 256.0, clicks: clicks }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                17 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(button)), Some(Argument::Uint(state)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Button { serial: serial, button: button, state: WEnum::from(state as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                18 => {
                    if let (Some(Argument::Uint(time)),) = (arg_iter.next(),) {
                        Ok((me, Event::Frame { time: time }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetCursor { serial, surface, hotspot_x, hotspot_y } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Uint(serial));
                    msg.args.push(Argument::Object(surface.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                    msg.args.push(Argument::Int(hotspot_x));
                    msg.args.push(Argument::Int(hotspot_y));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletToolV2 {
        #[doc = "set the tablet tool's surface\n\nSets the surface of the cursor used for this tool on the given\ntablet. This request only takes effect if the tool is in proximity\nof one of the requesting client's surfaces or the surface parameter\nis the current pointer surface. If there was a previous surface set\nwith this request it is replaced. If surface is NULL, the cursor\nimage is hidden.\n\nThe parameters hotspot_x and hotspot_y define the position of the\npointer surface relative to the pointer location. Its top-left corner\nis always at (x, y) - (hotspot_x, hotspot_y), where (x, y) are the\ncoordinates of the pointer location, in surface-local coordinates.\n\nOn surface.attach requests to the pointer surface, hotspot_x and\nhotspot_y are decremented by the x and y parameters passed to the\nrequest. Attach must be confirmed by wl_surface.commit as usual.\n\nThe hotspot can also be updated by passing the currently set pointer\nsurface to this request with new values for hotspot_x and hotspot_y.\n\nThe current and pending input regions of the wl_surface are cleared,\nand wl_surface.set_input_region is ignored until the wl_surface is no\nlonger used as the cursor. When the use as a cursor ends, the current\nand pending input regions become undefined, and the wl_surface is\nunmapped.\n\nThis request gives the surface the role of a zwp_tablet_tool_v2 cursor. A\nsurface may only ever be used as the cursor surface for one\nzwp_tablet_tool_v2. If the surface already has another role or has\npreviously been used as cursor surface for a different tool, a\nprotocol error is raised."]
        pub fn set_cursor(&self, serial: u32, surface: Option<&super::wl_surface::WlSurface>, hotspot_x: i32, hotspot_y: i32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetCursor { serial: serial, surface: surface.cloned(), hotspot_x: hotspot_x, hotspot_y: hotspot_y }, None);
        }
        #[doc = "destroy the tool object\n\nThis destroys the client's resource for this tool object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "graphics tablet device\n\nThe zwp_tablet_v2 interface represents one graphics tablet device. The\ntablet interface itself does not generate events; all events are\ngenerated by zwp_tablet_tool_v2 objects when in proximity above a tablet.\n\nA tablet has a number of static characteristics, e.g. device name and\npid/vid. These capabilities are sent in an event sequence after the\nzwp_tablet_seat_v2.tablet_added event. This initial event sequence is\nterminated by a zwp_tablet_v2.done event."]
        pub mod zwp_tablet_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "bus type \n\nDescribes the bus types this tablet is connected to."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Bustype {
        #[doc = "USB"]
            Usb = 3,
        #[doc = "Bluetooth"]
            Bluetooth = 5,
        #[doc = "Virtual"]
            Virtual = 6,
        #[doc = "Serial"]
            Serial = 17,
        #[doc = "I2C"]
            I2c = 24,
        }
        impl TryFrom<u32> for Bustype {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                3 => Ok(Self::Usb),
                5 => Ok(Self::Bluetooth),
                6 => Ok(Self::Virtual),
                17 => Ok(Self::Serial),
                24 => Ok(Self::I2c),
                _ => Err(()),
            } }
        }
        impl From<Bustype> for u32 { fn from(value: Bustype) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the tablet object\n\nThis destroys the client's resource for this tablet object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "tablet device name\n\nA descriptive name for the tablet device.\n\nIf the device has no descriptive name, this event is not sent.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_v2.done event."]
            Name {
        #[doc = "the device name"]
                name: String,
            },
        #[doc = "tablet device vendor/product id\n\nThe vendor and product IDs for the tablet device.\n\nThe interpretation of the id depends on the zwp_tablet_v2.bustype.\nPrior to version v2 of this protocol, the id was implied to be a USB\nvendor and product ID. If no zwp_tablet_v2.bustype is sent, the ID\nis to be interpreted as USB vendor and product ID.\n\nIf the device has no vendor/product ID, this event is not sent.\nThis can happen for virtual devices or non-USB devices, for instance.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_v2.done event."]
            Id {
        #[doc = "vendor id"]
                vid: u32,
        #[doc = "product id"]
                pid: u32,
            },
        #[doc = "path to the device\n\nA system-specific device path that indicates which device is behind\nthis zwp_tablet_v2. This information may be used to gather additional\ninformation about the device, e.g. through libwacom.\n\nA device may have more than one device path. If so, multiple\nzwp_tablet_v2.path events are sent. A device may be emulated and not\nhave a device path, and in that case this event will not be sent.\n\nThe format of the path is unspecified, it may be a device node, a\nsysfs path, or some other identifier. It is up to the client to\nidentify the string provided.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_v2.done event."]
            Path {
        #[doc = "path to local device"]
                path: String,
            },
        #[doc = "tablet description events sequence complete\n\nThis event is sent immediately to signal the end of the initial\nburst of descriptive events. A client may consider the static\ndescription of the tablet to be complete and finalize initialization\nof the tablet."]
            Done,
        #[doc = "tablet removed event\n\nSent when the tablet has been removed from the system. When a tablet\nis removed, some tools may be removed.\n\nWhen this event is received, the client must zwp_tablet_v2.destroy\nthe object."]
            Removed,
        #[doc = "tablet device bus type\n\nThe bustype argument is one of the BUS_ defines in the Linux kernel's\nlinux/input.h\n\nIf the device has no known bustype or the bustype cannot be\nqueried, this event is not sent.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_v2.done event."]
        #[doc = "Available since interface version 2."]
            Bustype {
        #[doc = "bus type"]
                bustype: WEnum<Bustype>,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Name { .. } => 0,
            Self::Id { .. } => 1,
            Self::Path { .. } => 2,
            Self::Done => 3,
            Self::Removed => 4,
            Self::Bustype { .. } => 5,
        } } }
        pub const EVT_NAME_SINCE: u32 = 1;
        pub const EVT_NAME_OPCODE: u16 = 0;
        pub const EVT_ID_SINCE: u32 = 1;
        pub const EVT_ID_OPCODE: u16 = 1;
        pub const EVT_PATH_SINCE: u32 = 1;
        pub const EVT_PATH_OPCODE: u16 = 2;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 3;
        pub const EVT_REMOVED_SINCE: u32 = 1;
        pub const EVT_REMOVED_OPCODE: u16 = 4;
        pub const EVT_BUSTYPE_SINCE: u32 = 2;
        pub const EVT_BUSTYPE_OPCODE: u16 = 5;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletV2 {}
        impl PartialEq<Weak<ZwpTabletV2>> for ZwpTabletV2 {
            fn eq(&self, other: &Weak<ZwpTabletV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Str(name)),) = (arg_iter.next(),) {
                        Ok((me, Event::Name { name: String::from_utf8_lossy(name.as_ref().unwrap().as_bytes()).into_owned() }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(vid)), Some(Argument::Uint(pid)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Id { vid: vid, pid: pid }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Str(path)),) = (arg_iter.next(),) {
                        Ok((me, Event::Path { path: String::from_utf8_lossy(path.as_ref().unwrap().as_bytes()).into_owned() }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                3 => Ok((me, Event::Done)),
                4 => Ok((me, Event::Removed)),
                5 => {
                    if let (Some(Argument::Uint(bustype)),) = (arg_iter.next(),) {
                        Ok((me, Event::Bustype { bustype: WEnum::from(bustype as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletV2 {
        #[doc = "destroy the tablet object\n\nThis destroys the client's resource for this tablet object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "pad ring\n\nA circular interaction area, such as the touch ring on the Wacom Intuos\nPro series tablets.\n\nEvents on a ring are logically grouped by the zwp_tablet_pad_ring_v2.frame\nevent."]
        pub mod zwp_tablet_pad_ring_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "ring axis source\n\nDescribes the source types for ring events. This indicates to the\nclient how a ring event was physically generated; a client may\nadjust the user interface accordingly. For example, events\nfrom a \"finger\" source may trigger kinetic scrolling."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Source {
        #[doc = "finger"]
            Finger = 1,
        }
        impl TryFrom<u32> for Source {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::Finger),
                _ => Err(()),
            } }
        }
        impl From<Source> for u32 { fn from(value: Source) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set compositor feedback\n\nRequest that the compositor use the provided feedback string\nassociated with this ring. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the ring is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the ring; compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nring. Requests providing other serials than the most recent one will be\nignored."]
            SetFeedback {
        #[doc = "ring description"]
                description: String,
        #[doc = "serial of the mode switch event"]
                serial: u32,
            },
        #[doc = "destroy the ring object\n\nThis destroys the client's resource for this ring object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetFeedback { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_FEEDBACK_SINCE: u32 = 1;
        pub const REQ_SET_FEEDBACK_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "ring event source\n\nSource information for ring events.\n\nThis event does not occur on its own. It is sent before a\nzwp_tablet_pad_ring_v2.frame event and carries the source information\nfor all events within that frame.\n\nThe source specifies how this event was generated. If the source is\nzwp_tablet_pad_ring_v2.source.finger, a zwp_tablet_pad_ring_v2.stop event\nwill be sent when the user lifts the finger off the device.\n\nThis event is optional. If the source is unknown for an interaction,\nno event is sent."]
            Source {
        #[doc = "the event source"]
                source: WEnum<Source>,
            },
        #[doc = "angle changed\n\nSent whenever the angle on a ring changes.\n\nThe angle is provided in degrees clockwise from the logical\nnorth of the ring in the pad's current rotation."]
            Angle {
        #[doc = "the current angle in degrees"]
                degrees: f64,
            },
        #[doc = "interaction stopped\n\nStop notification for ring events.\n\nFor some zwp_tablet_pad_ring_v2.source types, a zwp_tablet_pad_ring_v2.stop\nevent is sent to notify a client that the interaction with the ring\nhas terminated. This enables the client to implement kinetic scrolling.\nSee the zwp_tablet_pad_ring_v2.source documentation for information on\nwhen this event may be generated.\n\nAny zwp_tablet_pad_ring_v2.angle events with the same source after this\nevent should be considered as the start of a new interaction."]
            Stop,
        #[doc = "end of a ring event sequence\n\nIndicates the end of a set of ring events that logically belong\ntogether. A client is expected to accumulate the data in all events\nwithin the frame before proceeding.\n\nAll zwp_tablet_pad_ring_v2 events before a zwp_tablet_pad_ring_v2.frame event belong\nlogically together. For example, on termination of a finger interaction\non a ring the compositor will send a zwp_tablet_pad_ring_v2.source event,\na zwp_tablet_pad_ring_v2.stop event and a zwp_tablet_pad_ring_v2.frame event.\n\nA zwp_tablet_pad_ring_v2.frame event is sent for every logical event\ngroup, even if the group only contains a single zwp_tablet_pad_ring_v2\nevent. Specifically, a client may get a sequence: angle, frame,\nangle, frame, etc."]
            Frame {
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Source { .. } => 0,
            Self::Angle { .. } => 1,
            Self::Stop => 2,
            Self::Frame { .. } => 3,
        } } }
        pub const EVT_SOURCE_SINCE: u32 = 1;
        pub const EVT_SOURCE_OPCODE: u16 = 0;
        pub const EVT_ANGLE_SINCE: u32 = 1;
        pub const EVT_ANGLE_OPCODE: u16 = 1;
        pub const EVT_STOP_SINCE: u32 = 1;
        pub const EVT_STOP_OPCODE: u16 = 2;
        pub const EVT_FRAME_SINCE: u32 = 1;
        pub const EVT_FRAME_OPCODE: u16 = 3;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletPadRingV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletPadRingV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletPadRingV2 {}
        impl PartialEq<Weak<ZwpTabletPadRingV2>> for ZwpTabletPadRingV2 {
            fn eq(&self, other: &Weak<ZwpTabletPadRingV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletPadRingV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletPadRingV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletPadRingV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_PAD_RING_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(source)),) = (arg_iter.next(),) {
                        Ok((me, Event::Source { source: WEnum::from(source as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Fixed(degrees)),) = (arg_iter.next(),) {
                        Ok((me, Event::Angle { degrees: (degrees as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => Ok((me, Event::Stop)),
                3 => {
                    if let (Some(Argument::Uint(time)),) = (arg_iter.next(),) {
                        Ok((me, Event::Frame { time: time }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetFeedback { description, serial } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(description).unwrap()))));
                    msg.args.push(Argument::Uint(serial));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletPadRingV2 {
        #[doc = "set compositor feedback\n\nRequest that the compositor use the provided feedback string\nassociated with this ring. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the ring is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the ring; compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nring. Requests providing other serials than the most recent one will be\nignored."]
        pub fn set_feedback(&self, description: String, serial: u32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetFeedback { description: description, serial: serial }, None);
        }
        #[doc = "destroy the ring object\n\nThis destroys the client's resource for this ring object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "pad strip\n\nA linear interaction area, such as the strips found in Wacom Cintiq\nmodels.\n\nEvents on a strip are logically grouped by the zwp_tablet_pad_strip_v2.frame\nevent."]
        pub mod zwp_tablet_pad_strip_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "strip axis source\n\nDescribes the source types for strip events. This indicates to the\nclient how a strip event was physically generated; a client may\nadjust the user interface accordingly. For example, events\nfrom a \"finger\" source may trigger kinetic scrolling."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Source {
        #[doc = "finger"]
            Finger = 1,
        }
        impl TryFrom<u32> for Source {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::Finger),
                _ => Err(()),
            } }
        }
        impl From<Source> for u32 { fn from(value: Source) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this strip. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the strip is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the strip, and compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nstrip. Requests providing other serials than the most recent one will be\nignored."]
            SetFeedback {
        #[doc = "strip description"]
                description: String,
        #[doc = "serial of the mode switch event"]
                serial: u32,
            },
        #[doc = "destroy the strip object\n\nThis destroys the client's resource for this strip object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetFeedback { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_FEEDBACK_SINCE: u32 = 1;
        pub const REQ_SET_FEEDBACK_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "strip event source\n\nSource information for strip events.\n\nThis event does not occur on its own. It is sent before a\nzwp_tablet_pad_strip_v2.frame event and carries the source information\nfor all events within that frame.\n\nThe source specifies how this event was generated. If the source is\nzwp_tablet_pad_strip_v2.source.finger, a zwp_tablet_pad_strip_v2.stop event\nwill be sent when the user lifts their finger off the device.\n\nThis event is optional. If the source is unknown for an interaction,\nno event is sent."]
            Source {
        #[doc = "the event source"]
                source: WEnum<Source>,
            },
        #[doc = "position changed\n\nSent whenever the position on a strip changes.\n\nThe position is normalized to a range of [0, 65535], the 0-value\nrepresents the top-most and/or left-most position of the strip in\nthe pad's current rotation."]
            Position {
        #[doc = "the current position"]
                position: u32,
            },
        #[doc = "interaction stopped\n\nStop notification for strip events.\n\nFor some zwp_tablet_pad_strip_v2.source types, a zwp_tablet_pad_strip_v2.stop\nevent is sent to notify a client that the interaction with the strip\nhas terminated. This enables the client to implement kinetic\nscrolling. See the zwp_tablet_pad_strip_v2.source documentation for\ninformation on when this event may be generated.\n\nAny zwp_tablet_pad_strip_v2.position events with the same source after this\nevent should be considered as the start of a new interaction."]
            Stop,
        #[doc = "end of a strip event sequence\n\nIndicates the end of a set of events that represent one logical\nhardware strip event. A client is expected to accumulate the data\nin all events within the frame before proceeding.\n\nAll zwp_tablet_pad_strip_v2 events before a zwp_tablet_pad_strip_v2.frame event belong\nlogically together. For example, on termination of a finger interaction\non a strip the compositor will send a zwp_tablet_pad_strip_v2.source event,\na zwp_tablet_pad_strip_v2.stop event and a zwp_tablet_pad_strip_v2.frame\nevent.\n\nA zwp_tablet_pad_strip_v2.frame event is sent for every logical event\ngroup, even if the group only contains a single zwp_tablet_pad_strip_v2\nevent. Specifically, a client may get a sequence: position, frame,\nposition, frame, etc."]
            Frame {
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Source { .. } => 0,
            Self::Position { .. } => 1,
            Self::Stop => 2,
            Self::Frame { .. } => 3,
        } } }
        pub const EVT_SOURCE_SINCE: u32 = 1;
        pub const EVT_SOURCE_OPCODE: u16 = 0;
        pub const EVT_POSITION_SINCE: u32 = 1;
        pub const EVT_POSITION_OPCODE: u16 = 1;
        pub const EVT_STOP_SINCE: u32 = 1;
        pub const EVT_STOP_OPCODE: u16 = 2;
        pub const EVT_FRAME_SINCE: u32 = 1;
        pub const EVT_FRAME_OPCODE: u16 = 3;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletPadStripV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletPadStripV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletPadStripV2 {}
        impl PartialEq<Weak<ZwpTabletPadStripV2>> for ZwpTabletPadStripV2 {
            fn eq(&self, other: &Weak<ZwpTabletPadStripV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletPadStripV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletPadStripV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletPadStripV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_PAD_STRIP_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(source)),) = (arg_iter.next(),) {
                        Ok((me, Event::Source { source: WEnum::from(source as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(position)),) = (arg_iter.next(),) {
                        Ok((me, Event::Position { position: position }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => Ok((me, Event::Stop)),
                3 => {
                    if let (Some(Argument::Uint(time)),) = (arg_iter.next(),) {
                        Ok((me, Event::Frame { time: time }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetFeedback { description, serial } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(description).unwrap()))));
                    msg.args.push(Argument::Uint(serial));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletPadStripV2 {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this strip. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the strip is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the strip, and compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nstrip. Requests providing other serials than the most recent one will be\nignored."]
        pub fn set_feedback(&self, description: String, serial: u32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetFeedback { description: description, serial: serial }, None);
        }
        #[doc = "destroy the strip object\n\nThis destroys the client's resource for this strip object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "a set of buttons, rings and strips\n\nA pad group describes a distinct (sub)set of buttons, rings and strips\npresent in the tablet. The criteria of this grouping is usually positional,\neg. if a tablet has buttons on the left and right side, 2 groups will be\npresented. The physical arrangement of groups is undisclosed and may\nchange on the fly.\n\nPad groups will announce their features during pad initialization. Between\nthe corresponding zwp_tablet_pad_v2.group event and zwp_tablet_pad_group_v2.done, the\npad group will announce the buttons, rings and strips contained in it,\nplus the number of supported modes.\n\nModes are a mechanism to allow multiple groups of actions for every element\nin the pad group. The number of groups and available modes in each is\npersistent across device plugs. The current mode is user-switchable, it\nwill be announced through the zwp_tablet_pad_group_v2.mode_switch event both\nwhenever it is switched, and after zwp_tablet_pad_v2.enter.\n\nThe current mode logically applies to all elements in the pad group,\nalthough it is at clients' discretion whether to actually perform different\nactions, and/or issue the respective .set_feedback requests to notify the\ncompositor. See the zwp_tablet_pad_group_v2.mode_switch event for more details."]
        pub mod zwp_tablet_pad_group_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the pad object\n\nDestroy the zwp_tablet_pad_group_v2 object. Objects created from this object\nare unaffected and should be destroyed separately."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "buttons announced\n\nSent on zwp_tablet_pad_group_v2 initialization to announce the available\nbuttons in the group. Button indices start at 0, a button may only be\nin one group at a time.\n\nThis event is first sent in the initial burst of events before the\nzwp_tablet_pad_group_v2.done event.\n\nSome buttons are reserved by the compositor. These buttons may not be\nassigned to any zwp_tablet_pad_group_v2. Compositors may broadcast this\nevent in the case of changes to the mapping of these reserved buttons.\nIf the compositor happens to reserve all buttons in a group, this event\nwill be sent with an empty array."]
            Buttons {
        #[doc = "buttons in this group"]
                buttons: Vec<u8>,
            },
        #[doc = "ring announced\n\nSent on zwp_tablet_pad_group_v2 initialization to announce available rings.\nOne event is sent for each ring available on this pad group.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_group_v2.done event."]
            Ring {
                ring: super::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
            },
        #[doc = "strip announced\n\nSent on zwp_tablet_pad_v2 initialization to announce available strips.\nOne event is sent for each strip available on this pad group.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_group_v2.done event."]
            Strip {
                strip: super::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
            },
        #[doc = "mode-switch ability announced\n\nSent on zwp_tablet_pad_group_v2 initialization to announce that the pad\ngroup may switch between modes. A client may use a mode to store a\nspecific configuration for buttons, rings and strips and use the\nzwp_tablet_pad_group_v2.mode_switch event to toggle between these\nconfigurations. Mode indices start at 0.\n\nSwitching modes is compositor-dependent. See the\nzwp_tablet_pad_group_v2.mode_switch event for more details.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_group_v2.done event. This event is only sent when more than\nmore than one mode is available."]
            Modes {
        #[doc = "the number of modes"]
                modes: u32,
            },
        #[doc = "tablet group description events sequence complete\n\nThis event is sent immediately to signal the end of the initial\nburst of descriptive events. A client may consider the static\ndescription of the tablet to be complete and finalize initialization\nof the tablet group."]
            Done,
        #[doc = "mode switch event\n\nNotification that the mode was switched.\n\nA mode applies to all buttons, rings, strips and dials in a group\nsimultaneously, but a client is not required to assign different actions\nfor each mode. For example, a client may have mode-specific button\nmappings but map the ring to vertical scrolling in all modes. Mode\nindices start at 0.\n\nSwitching modes is compositor-dependent. The compositor may provide\nvisual cues to the user about the mode, e.g. by toggling LEDs on\nthe tablet device. Mode-switching may be software-controlled or\ncontrolled by one or more physical buttons. For example, on a Wacom\nIntuos Pro, the button inside the ring may be assigned to switch\nbetween modes.\n\nThe compositor will also send this event after zwp_tablet_pad_v2.enter on\neach group in order to notify of the current mode. Groups that only\nfeature one mode will use mode=0 when emitting this event.\n\nIf a button action in the new mode differs from the action in the\nprevious mode, the client should immediately issue a\nzwp_tablet_pad_v2.set_feedback request for each changed button.\n\nIf a ring, strip or dial action in the new mode differs from the action\nin the previous mode, the client should immediately issue a\nzwp_tablet_ring_v2.set_feedback, zwp_tablet_strip_v2.set_feedback or\nzwp_tablet_dial_v2.set_feedback request for each changed ring, strip or dial."]
            ModeSwitch {
        #[doc = "the time of the event with millisecond granularity"]
                time: u32,
                serial: u32,
        #[doc = "the new mode of the pad"]
                mode: u32,
            },
        #[doc = "dial announced\n\nSent on zwp_tablet_pad_v2 initialization to announce available dials.\nOne event is sent for each dial available on this pad group.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_group_v2.done event."]
        #[doc = "Available since interface version 2."]
            Dial {
                dial: super::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Buttons { .. } => 0,
            Self::Ring { .. } => 1,
            Self::Strip { .. } => 2,
            Self::Modes { .. } => 3,
            Self::Done => 4,
            Self::ModeSwitch { .. } => 5,
            Self::Dial { .. } => 6,
        } } }
        pub const EVT_BUTTONS_SINCE: u32 = 1;
        pub const EVT_BUTTONS_OPCODE: u16 = 0;
        pub const EVT_RING_SINCE: u32 = 1;
        pub const EVT_RING_OPCODE: u16 = 1;
        pub const EVT_STRIP_SINCE: u32 = 1;
        pub const EVT_STRIP_OPCODE: u16 = 2;
        pub const EVT_MODES_SINCE: u32 = 1;
        pub const EVT_MODES_OPCODE: u16 = 3;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 4;
        pub const EVT_MODE_SWITCH_SINCE: u32 = 1;
        pub const EVT_MODE_SWITCH_OPCODE: u16 = 5;
        pub const EVT_DIAL_SINCE: u32 = 2;
        pub const EVT_DIAL_OPCODE: u16 = 6;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletPadGroupV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletPadGroupV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletPadGroupV2 {}
        impl PartialEq<Weak<ZwpTabletPadGroupV2>> for ZwpTabletPadGroupV2 {
            fn eq(&self, other: &Weak<ZwpTabletPadGroupV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletPadGroupV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletPadGroupV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletPadGroupV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_PAD_GROUP_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Array(buttons)),) = (arg_iter.next(),) {
                        Ok((me, Event::Buttons { buttons: *buttons }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::NewId(ring)),) = (arg_iter.next(),) {
                        Ok((me, Event::Ring { ring: super::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2::from_id(conn, ring.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::NewId(strip)),) = (arg_iter.next(),) {
                        Ok((me, Event::Strip { strip: super::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2::from_id(conn, strip.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                3 => {
                    if let (Some(Argument::Uint(modes)),) = (arg_iter.next(),) {
                        Ok((me, Event::Modes { modes: modes }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                4 => Ok((me, Event::Done)),
                5 => {
                    if let (Some(Argument::Uint(time)), Some(Argument::Uint(serial)), Some(Argument::Uint(mode)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::ModeSwitch { time: time, serial: serial, mode: mode }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                6 => {
                    if let (Some(Argument::NewId(dial)),) = (arg_iter.next(),) {
                        Ok((me, Event::Dial { dial: super::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2::from_id(conn, dial.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletPadGroupV2 {
        #[doc = "destroy the pad object\n\nDestroy the zwp_tablet_pad_group_v2 object. Objects created from this object\nare unaffected and should be destroyed separately."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "a set of buttons, rings, strips and dials\n\nA pad device is a set of buttons, rings, strips and dials\nusually physically present on the tablet device itself. Some\nexceptions exist where the pad device is physically detached, e.g. the\nWacom ExpressKey Remote.\n\nPad devices have no axes that control the cursor and are generally\nauxiliary devices to the tool devices used on the tablet surface.\n\nA pad device has a number of static characteristics, e.g. the number\nof rings. These capabilities are sent in an event sequence after the\nzwp_tablet_seat_v2.pad_added event before any actual events from this pad.\nThis initial event sequence is terminated by a zwp_tablet_pad_v2.done\nevent.\n\nAll pad features (buttons, rings, strips and dials) are logically divided into\ngroups and all pads have at least one group. The available groups are\nnotified through the zwp_tablet_pad_v2.group event; the compositor will\nemit one event per group before emitting zwp_tablet_pad_v2.done.\n\nGroups may have multiple modes. Modes allow clients to map multiple\nactions to a single pad feature. Only one mode can be active per group,\nalthough different groups may have different active modes."]
        pub mod zwp_tablet_pad_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "physical button state\n\nDescribes the physical state of a button that caused the button\nevent."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum ButtonState {
        #[doc = "the button is not pressed"]
            Released = 0,
        #[doc = "the button is pressed"]
            Pressed = 1,
        }
        impl TryFrom<u32> for ButtonState {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::Released),
                1 => Ok(Self::Pressed),
                _ => Err(()),
            } }
        }
        impl From<ButtonState> for u32 { fn from(value: ButtonState) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this button. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever a button is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with each button, and compositors may use\nthis information to offer visual feedback on the button layout\n(e.g. on-screen displays).\n\nButton indices start at 0. Setting the feedback string on a button\nthat is reserved by the compositor (i.e. not belonging to any\nzwp_tablet_pad_group_v2) does not generate an error but the compositor\nis free to ignore the request.\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nbutton. Requests providing other serials than the most recent one will\nbe ignored."]
            SetFeedback {
        #[doc = "button index"]
                button: u32,
        #[doc = "button description"]
                description: String,
        #[doc = "serial of the mode switch event"]
                serial: u32,
            },
        #[doc = "destroy the pad object\n\nDestroy the zwp_tablet_pad_v2 object. Objects created from this object\nare unaffected and should be destroyed separately."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetFeedback { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_FEEDBACK_SINCE: u32 = 1;
        pub const REQ_SET_FEEDBACK_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "group announced\n\nSent on zwp_tablet_pad_v2 initialization to announce available groups.\nOne event is sent for each pad group available.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_v2.done event. At least one group will be announced."]
            Group {
                pad_group: super::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2,
            },
        #[doc = "path to the device\n\nA system-specific device path that indicates which device is behind\nthis zwp_tablet_pad_v2. This information may be used to gather additional\ninformation about the device, e.g. through libwacom.\n\nThe format of the path is unspecified, it may be a device node, a\nsysfs path, or some other identifier. It is up to the client to\nidentify the string provided.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_v2.done event."]
            Path {
        #[doc = "path to local device"]
                path: String,
            },
        #[doc = "buttons announced\n\nSent on zwp_tablet_pad_v2 initialization to announce the available\nbuttons.\n\nThis event is sent in the initial burst of events before the\nzwp_tablet_pad_v2.done event. This event is only sent when at least one\nbutton is available."]
            Buttons {
        #[doc = "the number of buttons"]
                buttons: u32,
            },
        #[doc = "pad description event sequence complete\n\nThis event signals the end of the initial burst of descriptive\nevents. A client may consider the static description of the pad to\nbe complete and finalize initialization of the pad."]
            Done,
        #[doc = "physical button state\n\nSent whenever the physical state of a button changes."]
            Button {
        #[doc = "the time of the event with millisecond granularity"]
                time: u32,
        #[doc = "the index of the button that changed state"]
                button: u32,
                state: WEnum<ButtonState>,
            },
        #[doc = "enter event\n\nNotification that this pad is focused on the specified surface."]
            Enter {
        #[doc = "serial number of the enter event"]
                serial: u32,
        #[doc = "the tablet the pad is attached to"]
                tablet: super::zwp_tablet_v2::ZwpTabletV2,
        #[doc = "surface the pad is focused on"]
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "leave event\n\nNotification that this pad is no longer focused on the specified\nsurface."]
            Leave {
        #[doc = "serial number of the leave event"]
                serial: u32,
        #[doc = "surface the pad is no longer focused on"]
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "pad removed event\n\nSent when the pad has been removed from the system. When a tablet\nis removed its pad(s) will be removed too.\n\nWhen this event is received, the client must destroy all rings, strips\nand groups that were offered by this pad, and issue zwp_tablet_pad_v2.destroy\nthe pad itself."]
            Removed,
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Group { .. } => 0,
            Self::Path { .. } => 1,
            Self::Buttons { .. } => 2,
            Self::Done => 3,
            Self::Button { .. } => 4,
            Self::Enter { .. } => 5,
            Self::Leave { .. } => 6,
            Self::Removed => 7,
        } } }
        pub const EVT_GROUP_SINCE: u32 = 1;
        pub const EVT_GROUP_OPCODE: u16 = 0;
        pub const EVT_PATH_SINCE: u32 = 1;
        pub const EVT_PATH_OPCODE: u16 = 1;
        pub const EVT_BUTTONS_SINCE: u32 = 1;
        pub const EVT_BUTTONS_OPCODE: u16 = 2;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 3;
        pub const EVT_BUTTON_SINCE: u32 = 1;
        pub const EVT_BUTTON_OPCODE: u16 = 4;
        pub const EVT_ENTER_SINCE: u32 = 1;
        pub const EVT_ENTER_OPCODE: u16 = 5;
        pub const EVT_LEAVE_SINCE: u32 = 1;
        pub const EVT_LEAVE_OPCODE: u16 = 6;
        pub const EVT_REMOVED_SINCE: u32 = 1;
        pub const EVT_REMOVED_OPCODE: u16 = 7;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletPadV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletPadV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletPadV2 {}
        impl PartialEq<Weak<ZwpTabletPadV2>> for ZwpTabletPadV2 {
            fn eq(&self, other: &Weak<ZwpTabletPadV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletPadV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletPadV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletPadV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_PAD_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::NewId(pad_group)),) = (arg_iter.next(),) {
                        Ok((me, Event::Group { pad_group: super::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2::from_id(conn, pad_group.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Str(path)),) = (arg_iter.next(),) {
                        Ok((me, Event::Path { path: String::from_utf8_lossy(path.as_ref().unwrap().as_bytes()).into_owned() }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Uint(buttons)),) = (arg_iter.next(),) {
                        Ok((me, Event::Buttons { buttons: buttons }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                3 => Ok((me, Event::Done)),
                4 => {
                    if let (Some(Argument::Uint(time)), Some(Argument::Uint(button)), Some(Argument::Uint(state)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Button { time: time, button: button, state: WEnum::from(state as u32) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                5 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Object(tablet)), Some(Argument::Object(surface)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Enter { serial: serial, tablet: super::zwp_tablet_v2::ZwpTabletV2::from_id(conn, tablet.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                6 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Object(surface)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Leave { serial: serial, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                7 => Ok((me, Event::Removed)),
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetFeedback { button, description, serial } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Uint(button));
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(description).unwrap()))));
                    msg.args.push(Argument::Uint(serial));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletPadV2 {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this button. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever a button is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with each button, and compositors may use\nthis information to offer visual feedback on the button layout\n(e.g. on-screen displays).\n\nButton indices start at 0. Setting the feedback string on a button\nthat is reserved by the compositor (i.e. not belonging to any\nzwp_tablet_pad_group_v2) does not generate an error but the compositor\nis free to ignore the request.\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\nbutton. Requests providing other serials than the most recent one will\nbe ignored."]
        pub fn set_feedback(&self, button: u32, description: String, serial: u32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetFeedback { button: button, description: description, serial: serial }, None);
        }
        #[doc = "destroy the pad object\n\nDestroy the zwp_tablet_pad_v2 object. Objects created from this object\nare unaffected and should be destroyed separately."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "pad dial\n\nA rotary control, e.g. a dial or a wheel.\n\nEvents on a dial are logically grouped by the zwp_tablet_pad_dial_v2.frame\nevent."]
        pub mod zwp_tablet_pad_dial_v2 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this dial. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the dial is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the dial, and compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\ndial. Requests providing other serials than the most recent one will be\nignored."]
            SetFeedback {
        #[doc = "dial description"]
                description: String,
        #[doc = "serial of the mode switch event"]
                serial: u32,
            },
        #[doc = "destroy the dial object\n\nThis destroys the client's resource for this dial object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetFeedback { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_FEEDBACK_SINCE: u32 = 1;
        pub const REQ_SET_FEEDBACK_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "delta movement\n\nSent whenever the position on a dial changes.\n\nThis event carries the wheel delta as multiples or fractions\nof 120 with each multiple of 120 representing one logical wheel detent.\nFor example, an axis_value120 of 30 is one quarter of\na logical wheel step in the positive direction, a value120 of\n-240 are two logical wheel steps in the negative direction within the\nsame hardware event. See the wl_pointer.axis_value120 for more details.\n\nThe value120 must not be zero."]
            Delta {
        #[doc = "rotation distance as fraction of 120"]
                value120: i32,
            },
        #[doc = "end of a dial event sequence\n\nIndicates the end of a set of events that represent one logical\nhardware dial event. A client is expected to accumulate the data\nin all events within the frame before proceeding.\n\nAll zwp_tablet_pad_dial_v2 events before a zwp_tablet_pad_dial_v2.frame event belong\nlogically together.\n\nA zwp_tablet_pad_dial_v2.frame event is sent for every logical event\ngroup, even if the group only contains a single zwp_tablet_pad_dial_v2\nevent. Specifically, a client may get a sequence: delta, frame,\ndelta, frame, etc."]
            Frame {
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Delta { .. } => 0,
            Self::Frame { .. } => 1,
        } } }
        pub const EVT_DELTA_SINCE: u32 = 1;
        pub const EVT_DELTA_OPCODE: u16 = 0;
        pub const EVT_FRAME_SINCE: u32 = 1;
        pub const EVT_FRAME_OPCODE: u16 = 1;

        #[derive(Debug, Clone)]
        pub struct ZwpTabletPadDialV2 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTabletPadDialV2 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTabletPadDialV2 {}
        impl PartialEq<Weak<ZwpTabletPadDialV2>> for ZwpTabletPadDialV2 {
            fn eq(&self, other: &Weak<ZwpTabletPadDialV2>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTabletPadDialV2 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTabletPadDialV2 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTabletPadDialV2 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TABLET_PAD_DIAL_V2_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Int(value120)),) = (arg_iter.next(),) {
                        Ok((me, Event::Delta { value120: value120 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(time)),) = (arg_iter.next(),) {
                        Ok((me, Event::Frame { time: time }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetFeedback { description, serial } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(description).unwrap()))));
                    msg.args.push(Argument::Uint(serial));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTabletPadDialV2 {
        #[doc = "set compositor feedback\n\nRequests the compositor to use the provided feedback string\nassociated with this dial. This request should be issued immediately\nafter a zwp_tablet_pad_group_v2.mode_switch event from the corresponding\ngroup is received, or whenever the dial is mapped to a different\naction. See zwp_tablet_pad_group_v2.mode_switch for more details.\n\nClients are encouraged to provide context-aware descriptions for\nthe actions associated with the dial, and compositors may use this\ninformation to offer visual feedback about the button layout\n(eg. on-screen displays).\n\nThe provided string 'description' is a UTF-8 encoded string to be\nassociated with this ring, and is considered user-visible; general\ninternationalization rules apply.\n\nThe serial argument will be that of the last\nzwp_tablet_pad_group_v2.mode_switch event received for the group of this\ndial. Requests providing other serials than the most recent one will be\nignored."]
        pub fn set_feedback(&self, description: String, serial: u32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetFeedback { description: description, serial: serial }, None);
        }
        #[doc = "destroy the dial object\n\nThis destroys the client's resource for this dial object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }

        }
    }
}

#[cfg(all(feature = "staging", feature = "unstable"))]
pub mod cursor_shape {
    pub mod v1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright 2018 The Chromium Authors
        // Copyright 2023 Simon Ser
        //
        // Permission is hereby granted, free of charge, to any person obtaining a
        // copy of this software and associated documentation files (the "Software"),
        // to deal in the Software without restriction, including without limitation
        // the rights to use, copy, modify, merge, publish, distribute, sublicense,
        // and/or sell copies of the Software, and to permit persons to whom the
        // Software is furnished to do so, subject to the following conditions:
        // The above copyright notice and this permission notice (including the next
        // paragraph) shall be included in all copies or substantial portions of the
        // Software.
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
        // THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
        // FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
        // DEALINGS IN THE SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            use crate::wp::tablet::zv2::{client::*};
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                use crate::wp::tablet::zv2::{client::__interfaces::*};
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static WP_CURSOR_SHAPE_MANAGER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "wp_cursor_shape_manager_v1", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_pointer", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&WP_CURSOR_SHAPE_DEVICE_V1_INTERFACE), arg_interfaces: &[&WL_POINTER_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "get_tablet_tool_v2", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&WP_CURSOR_SHAPE_DEVICE_V1_INTERFACE), arg_interfaces: &[&ZWP_TABLET_TOOL_V2_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&wp_cursor_shape_manager_v1_interface),
        };
        static wp_cursor_shape_manager_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static wp_cursor_shape_manager_v1_requests_get_pointer_types: InterfacePointers = InterfacePointers(&[&wp_cursor_shape_device_v1_interface,&wl_pointer_interface,]);
        static wp_cursor_shape_manager_v1_requests_get_tablet_tool_v2_types: InterfacePointers = InterfacePointers(&[&wp_cursor_shape_device_v1_interface,&zwp_tablet_tool_v2_interface,]);
        static wp_cursor_shape_manager_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_cursor_shape_manager_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_pointer".as_ptr(), signature: c"no".as_ptr(), types: wp_cursor_shape_manager_v1_requests_get_pointer_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_tablet_tool_v2".as_ptr(), signature: c"no".as_ptr(), types: wp_cursor_shape_manager_v1_requests_get_tablet_tool_v2_types.0.as_ptr() },
        ]);
        pub static wp_cursor_shape_manager_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"wp_cursor_shape_manager_v1".as_ptr(), version: 2, request_count: 3, requests: wp_cursor_shape_manager_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static WP_CURSOR_SHAPE_DEVICE_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "wp_cursor_shape_device_v1", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_shape", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&wp_cursor_shape_device_v1_interface),
        };
        static wp_cursor_shape_device_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static wp_cursor_shape_device_v1_requests_set_shape_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static wp_cursor_shape_device_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_cursor_shape_device_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_shape".as_ptr(), signature: c"uu".as_ptr(), types: wp_cursor_shape_device_v1_requests_set_shape_types.0.as_ptr() },
        ]);
        pub static wp_cursor_shape_device_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"wp_cursor_shape_device_v1".as_ptr(), version: 2, request_count: 2, requests: wp_cursor_shape_device_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };

            }
            use self::__interfaces::*;
            #[doc = "cursor shape manager\n\nThis global offers an alternative, optional way to set cursor images. This\nnew way uses enumerated cursors instead of a wl_surface like\nwl_pointer.set_cursor does.\n\nWarning! The protocol described in this file is currently in the testing\nphase. Backward compatible changes may be added together with the\ncorresponding interface version bump. Backward incompatible changes can\nonly be done by creating a new major version of the extension."]
        pub mod wp_cursor_shape_manager_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the manager\n\nDestroy the cursor shape manager."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "manage the cursor shape of a pointer device\n\nObtain a wp_cursor_shape_device_v1 for a wl_pointer object.\n\nWhen the pointer capability is removed from the wl_seat, the\nwp_cursor_shape_device_v1 object becomes inert."]
            GetPointer {
                pointer: super::wl_pointer::WlPointer,
            },
        #[doc = "manage the cursor shape of a tablet tool device\n\nObtain a wp_cursor_shape_device_v1 for a zwp_tablet_tool_v2 object.\n\nWhen the zwp_tablet_tool_v2 is removed, the wp_cursor_shape_device_v1\nobject becomes inert."]
            GetTabletToolV2 {
                tablet_tool: super::zwp_tablet_tool_v2::ZwpTabletToolV2,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::GetPointer { .. } => 1,
            Self::GetTabletToolV2 { .. } => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_GET_POINTER_SINCE: u32 = 1;
        pub const REQ_GET_POINTER_OPCODE: u16 = 1;
        pub const REQ_GET_TABLET_TOOL_V2_SINCE: u32 = 1;
        pub const REQ_GET_TABLET_TOOL_V2_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct WpCursorShapeManagerV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for WpCursorShapeManagerV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for WpCursorShapeManagerV1 {}
        impl PartialEq<Weak<WpCursorShapeManagerV1>> for WpCursorShapeManagerV1 {
            fn eq(&self, other: &Weak<WpCursorShapeManagerV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for WpCursorShapeManagerV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for WpCursorShapeManagerV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for WpCursorShapeManagerV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::WP_CURSOR_SHAPE_MANAGER_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::GetPointer { pointer } => {
                    let child_spec = Some((super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(pointer.id()));
                    Ok((msg, child_spec))
                },
                Request::GetTabletToolV2 { tablet_tool } => {
                    let child_spec = Some((super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(tablet_tool.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl WpCursorShapeManagerV1 {
        #[doc = "destroy the manager\n\nDestroy the cursor shape manager."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "manage the cursor shape of a pointer device\n\nObtain a wp_cursor_shape_device_v1 for a wl_pointer object.\n\nWhen the pointer capability is removed from the wl_seat, the\nwp_cursor_shape_device_v1 object becomes inert."]
        pub fn get_pointer<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, U> + 'static>(&self, pointer: &super::wl_pointer::WlPointer, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1 {
            self.send_constructor::<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>(Request::GetPointer { pointer: pointer.clone() }, qh.make_data::<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, U>(udata)).unwrap_or_else(|_| super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1::inert(self.backend.clone()))
        }
        #[doc = "manage the cursor shape of a tablet tool device\n\nObtain a wp_cursor_shape_device_v1 for a zwp_tablet_tool_v2 object.\n\nWhen the zwp_tablet_tool_v2 is removed, the wp_cursor_shape_device_v1\nobject becomes inert."]
        pub fn get_tablet_tool_v2<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, U> + 'static>(&self, tablet_tool: &super::zwp_tablet_tool_v2::ZwpTabletToolV2, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1 {
            self.send_constructor::<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>(Request::GetTabletToolV2 { tablet_tool: tablet_tool.clone() }, qh.make_data::<super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, U>(udata)).unwrap_or_else(|_| super::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1::inert(self.backend.clone()))
        }
        }
        }
        #[doc = "cursor shape for a device\n\nThis interface allows clients to set the cursor shape."]
        pub mod wp_cursor_shape_device_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "cursor shapes\n\nThis enum describes cursor shapes.\n\nThe names are taken from the CSS W3C specification:\nhttps://w3c.github.io/csswg-drafts/css-ui/#cursor\nwith a few additions.\n\nNote that there are some groups of cursor shapes that are related:\nThe first group is drag-and-drop cursors which are used to indicate\nthe selected action during dnd operations. The second group is resize\ncursors which are used to indicate resizing and moving possibilities\non window borders. It is recommended that the shapes in these groups\nshould use visually compatible images and metaphors."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Shape {
        #[doc = "default cursor"]
            Default = 1,
        #[doc = "a context menu is available for the object under the cursor"]
            ContextMenu = 2,
        #[doc = "help is available for the object under the cursor"]
            Help = 3,
        #[doc = "pointer that indicates a link or another interactive element"]
            Pointer = 4,
        #[doc = "progress indicator"]
            Progress = 5,
        #[doc = "program is busy, user should wait"]
            Wait = 6,
        #[doc = "a cell or set of cells may be selected"]
            Cell = 7,
        #[doc = "simple crosshair"]
            Crosshair = 8,
        #[doc = "text may be selected"]
            Text = 9,
        #[doc = "vertical text may be selected"]
            VerticalText = 10,
        #[doc = "drag-and-drop: alias of/shortcut to something is to be created"]
            Alias = 11,
        #[doc = "drag-and-drop: something is to be copied"]
            Copy = 12,
        #[doc = "drag-and-drop: something is to be moved"]
            Move = 13,
        #[doc = "drag-and-drop: the dragged item cannot be dropped at the current cursor location"]
            NoDrop = 14,
        #[doc = "drag-and-drop: the requested action will not be carried out"]
            NotAllowed = 15,
        #[doc = "drag-and-drop: something can be grabbed"]
            Grab = 16,
        #[doc = "drag-and-drop: something is being grabbed"]
            Grabbing = 17,
        #[doc = "resizing: the east border is to be moved"]
            EResize = 18,
        #[doc = "resizing: the north border is to be moved"]
            NResize = 19,
        #[doc = "resizing: the north-east corner is to be moved"]
            NeResize = 20,
        #[doc = "resizing: the north-west corner is to be moved"]
            NwResize = 21,
        #[doc = "resizing: the south border is to be moved"]
            SResize = 22,
        #[doc = "resizing: the south-east corner is to be moved"]
            SeResize = 23,
        #[doc = "resizing: the south-west corner is to be moved"]
            SwResize = 24,
        #[doc = "resizing: the west border is to be moved"]
            WResize = 25,
        #[doc = "resizing: the east and west borders are to be moved"]
            EwResize = 26,
        #[doc = "resizing: the north and south borders are to be moved"]
            NsResize = 27,
        #[doc = "resizing: the north-east and south-west corners are to be moved"]
            NeswResize = 28,
        #[doc = "resizing: the north-west and south-east corners are to be moved"]
            NwseResize = 29,
        #[doc = "resizing: that the item/column can be resized horizontally"]
            ColResize = 30,
        #[doc = "resizing: that the item/row can be resized vertically"]
            RowResize = 31,
        #[doc = "something can be scrolled in any direction"]
            AllScroll = 32,
        #[doc = "something can be zoomed in"]
            ZoomIn = 33,
        #[doc = "something can be zoomed out"]
            ZoomOut = 34,
        #[doc = "drag-and-drop: the user will select which action will be carried out (non-css value)"]
            DndAsk = 35,
        #[doc = "resizing: something can be moved or resized in any direction (non-css value)"]
            AllResize = 36,
        }
        impl TryFrom<u32> for Shape {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::Default),
                2 => Ok(Self::ContextMenu),
                3 => Ok(Self::Help),
                4 => Ok(Self::Pointer),
                5 => Ok(Self::Progress),
                6 => Ok(Self::Wait),
                7 => Ok(Self::Cell),
                8 => Ok(Self::Crosshair),
                9 => Ok(Self::Text),
                10 => Ok(Self::VerticalText),
                11 => Ok(Self::Alias),
                12 => Ok(Self::Copy),
                13 => Ok(Self::Move),
                14 => Ok(Self::NoDrop),
                15 => Ok(Self::NotAllowed),
                16 => Ok(Self::Grab),
                17 => Ok(Self::Grabbing),
                18 => Ok(Self::EResize),
                19 => Ok(Self::NResize),
                20 => Ok(Self::NeResize),
                21 => Ok(Self::NwResize),
                22 => Ok(Self::SResize),
                23 => Ok(Self::SeResize),
                24 => Ok(Self::SwResize),
                25 => Ok(Self::WResize),
                26 => Ok(Self::EwResize),
                27 => Ok(Self::NsResize),
                28 => Ok(Self::NeswResize),
                29 => Ok(Self::NwseResize),
                30 => Ok(Self::ColResize),
                31 => Ok(Self::RowResize),
                32 => Ok(Self::AllScroll),
                33 => Ok(Self::ZoomIn),
                34 => Ok(Self::ZoomOut),
                35 => Ok(Self::DndAsk),
                36 => Ok(Self::AllResize),
                _ => Err(()),
            } }
        }
        impl From<Shape> for u32 { fn from(value: Shape) -> u32 { value as u32 } }
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Error {
        #[doc = "the specified shape value is invalid"]
            InvalidShape = 1,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::InvalidShape),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the cursor shape device\n\nDestroy the cursor shape device.\n\nThe device cursor shape remains unchanged."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "set device cursor to the shape\n\nSets the device cursor to the specified shape. The compositor will\nchange the cursor image based on the specified shape.\n\nThe cursor actually changes only if the input device focus is one of\nthe requesting client's surfaces. If any, the previous cursor image\n(surface or shape) is replaced.\n\nThe \"shape\" argument must be a valid enum entry, otherwise the\ninvalid_shape protocol error is raised.\n\nThis is similar to the wl_pointer.set_cursor and\nzwp_tablet_tool_v2.set_cursor requests, but this request accepts a\nshape instead of contents in the form of a surface. Clients can mix\nset_cursor and set_shape requests.\n\nThe serial parameter must match the latest wl_pointer.enter or\nzwp_tablet_tool_v2.proximity_in serial number sent to the client.\nOtherwise the request will be ignored."]
            SetShape {
        #[doc = "serial number of the enter event"]
                serial: u32,
                shape: WEnum<Shape>,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::SetShape { .. } => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_SET_SHAPE_SINCE: u32 = 1;
        pub const REQ_SET_SHAPE_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct WpCursorShapeDeviceV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for WpCursorShapeDeviceV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for WpCursorShapeDeviceV1 {}
        impl PartialEq<Weak<WpCursorShapeDeviceV1>> for WpCursorShapeDeviceV1 {
            fn eq(&self, other: &Weak<WpCursorShapeDeviceV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for WpCursorShapeDeviceV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for WpCursorShapeDeviceV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for WpCursorShapeDeviceV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::WP_CURSOR_SHAPE_DEVICE_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::SetShape { serial, shape } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::Uint(serial));
                    msg.args.push(Argument::Uint(u32::from(shape)));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl WpCursorShapeDeviceV1 {
        #[doc = "destroy the cursor shape device\n\nDestroy the cursor shape device.\n\nThe device cursor shape remains unchanged."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "set device cursor to the shape\n\nSets the device cursor to the specified shape. The compositor will\nchange the cursor image based on the specified shape.\n\nThe cursor actually changes only if the input device focus is one of\nthe requesting client's surfaces. If any, the previous cursor image\n(surface or shape) is replaced.\n\nThe \"shape\" argument must be a valid enum entry, otherwise the\ninvalid_shape protocol error is raised.\n\nThis is similar to the wl_pointer.set_cursor and\nzwp_tablet_tool_v2.set_cursor requests, but this request accepts a\nshape instead of contents in the form of a surface. Clients can mix\nset_cursor and set_shape requests.\n\nThe serial parameter must match the latest wl_pointer.enter or\nzwp_tablet_tool_v2.proximity_in serial number sent to the client.\nOtherwise the request will be ignored."]
        pub fn set_shape(&self, serial: u32, shape: Shape) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetShape { serial: serial, shape: WEnum::Value(shape) }, None);
        }
        }
        }

        }
    }
}

#[cfg(feature = "staging")]
pub mod fractional_scale {
    pub mod v1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2022 Kenny Levinsen
        //
        // Permission is hereby granted, free of charge, to any person obtaining a
        // copy of this software and associated documentation files (the "Software"),
        // to deal in the Software without restriction, including without limitation
        // the rights to use, copy, modify, merge, publish, distribute, sublicense,
        // and/or sell copies of the Software, and to permit persons to whom the
        // Software is furnished to do so, subject to the following conditions:
        //
        // The above copyright notice and this permission notice (including the next
        // paragraph) shall be included in all copies or substantial portions of the
        // Software.
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
        // THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
        // FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
        // DEALINGS IN THE SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static WP_FRACTIONAL_SCALE_MANAGER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "wp_fractional_scale_manager_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_fractional_scale", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&WP_FRACTIONAL_SCALE_V1_INTERFACE), arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&wp_fractional_scale_manager_v1_interface),
        };
        static wp_fractional_scale_manager_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static wp_fractional_scale_manager_v1_requests_get_fractional_scale_types: InterfacePointers = InterfacePointers(&[&wp_fractional_scale_v1_interface,&wl_surface_interface,]);
        static wp_fractional_scale_manager_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_fractional_scale_manager_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_fractional_scale".as_ptr(), signature: c"no".as_ptr(), types: wp_fractional_scale_manager_v1_requests_get_fractional_scale_types.0.as_ptr() },
        ]);
        pub static wp_fractional_scale_manager_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"wp_fractional_scale_manager_v1".as_ptr(), version: 1, request_count: 2, requests: wp_fractional_scale_manager_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static WP_FRACTIONAL_SCALE_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "wp_fractional_scale_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "preferred_scale", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&wp_fractional_scale_v1_interface),
        };
        static wp_fractional_scale_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static wp_fractional_scale_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_fractional_scale_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static wp_fractional_scale_v1_events_preferred_scale_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static wp_fractional_scale_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"preferred_scale".as_ptr(), signature: c"u".as_ptr(), types: wp_fractional_scale_v1_events_preferred_scale_types.0.as_ptr() },
        ]);
        pub static wp_fractional_scale_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"wp_fractional_scale_v1".as_ptr(), version: 1, request_count: 1, requests: wp_fractional_scale_v1_requests.0.as_ptr(), event_count: 1, events: wp_fractional_scale_v1_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "fractional surface scale information\n\nA global interface for requesting surfaces to use fractional scales."]
        pub mod wp_fractional_scale_manager_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Error {
        #[doc = "the surface already has a fractional_scale object associated"]
            FractionalScaleExists = 0,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::FractionalScaleExists),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "unbind the fractional surface scale interface\n\nInforms the server that the client will not be using this protocol\nobject anymore. This does not affect any other objects,\nwp_fractional_scale_v1 objects included."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "extend surface interface for scale information\n\nCreate an add-on object for the the wl_surface to let the compositor\nrequest fractional scales. If the given wl_surface already has a\nwp_fractional_scale_v1 object associated, the fractional_scale_exists\nprotocol error is raised."]
            GetFractionalScale {
        #[doc = "the surface"]
                surface: super::wl_surface::WlSurface,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::GetFractionalScale { .. } => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_GET_FRACTIONAL_SCALE_SINCE: u32 = 1;
        pub const REQ_GET_FRACTIONAL_SCALE_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct WpFractionalScaleManagerV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for WpFractionalScaleManagerV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for WpFractionalScaleManagerV1 {}
        impl PartialEq<Weak<WpFractionalScaleManagerV1>> for WpFractionalScaleManagerV1 {
            fn eq(&self, other: &Weak<WpFractionalScaleManagerV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for WpFractionalScaleManagerV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for WpFractionalScaleManagerV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for WpFractionalScaleManagerV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::WP_FRACTIONAL_SCALE_MANAGER_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::GetFractionalScale { surface } => {
                    let child_spec = Some((super::wp_fractional_scale_v1::WpFractionalScaleV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(surface.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl WpFractionalScaleManagerV1 {
        #[doc = "unbind the fractional surface scale interface\n\nInforms the server that the client will not be using this protocol\nobject anymore. This does not affect any other objects,\nwp_fractional_scale_v1 objects included."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "extend surface interface for scale information\n\nCreate an add-on object for the the wl_surface to let the compositor\nrequest fractional scales. If the given wl_surface already has a\nwp_fractional_scale_v1 object associated, the fractional_scale_exists\nprotocol error is raised."]
        pub fn get_fractional_scale<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::wp_fractional_scale_v1::WpFractionalScaleV1, U> + 'static>(&self, surface: &super::wl_surface::WlSurface, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::wp_fractional_scale_v1::WpFractionalScaleV1 {
            self.send_constructor::<super::wp_fractional_scale_v1::WpFractionalScaleV1>(Request::GetFractionalScale { surface: surface.clone() }, qh.make_data::<super::wp_fractional_scale_v1::WpFractionalScaleV1, U>(udata)).unwrap_or_else(|_| super::wp_fractional_scale_v1::WpFractionalScaleV1::inert(self.backend.clone()))
        }
        }
        }
        #[doc = "fractional scale interface to a wl_surface\n\nAn additional interface to a wl_surface object which allows the compositor\nto inform the client of the preferred scale."]
        pub mod wp_fractional_scale_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "remove surface scale information for surface\n\nDestroy the fractional scale object. When this object is destroyed,\npreferred_scale events will no longer be sent."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "notify of new preferred scale\n\nNotification of a new preferred scale for this surface that the\ncompositor suggests that the client should use.\n\nThe sent scale is the numerator of a fraction with a denominator of 120."]
            PreferredScale {
        #[doc = "the new preferred scale"]
                scale: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::PreferredScale { .. } => 0,
        } } }
        pub const EVT_PREFERRED_SCALE_SINCE: u32 = 1;
        pub const EVT_PREFERRED_SCALE_OPCODE: u16 = 0;

        #[derive(Debug, Clone)]
        pub struct WpFractionalScaleV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for WpFractionalScaleV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for WpFractionalScaleV1 {}
        impl PartialEq<Weak<WpFractionalScaleV1>> for WpFractionalScaleV1 {
            fn eq(&self, other: &Weak<WpFractionalScaleV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for WpFractionalScaleV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for WpFractionalScaleV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for WpFractionalScaleV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::WP_FRACTIONAL_SCALE_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(scale)),) = (arg_iter.next(),) {
                        Ok((me, Event::PreferredScale { scale: scale }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl WpFractionalScaleV1 {
        #[doc = "remove surface scale information for surface\n\nDestroy the fractional scale object. When this object is destroyed,\npreferred_scale events will no longer be sent."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }

        }
    }
}

#[cfg(feature = "unstable")]
pub mod text_input {
    pub mod zv3 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2012, 2013 Intel Corporation
        // Copyright © 2015, 2016 Jan Arne Petersen
        // Copyright © 2017, 2018 Red Hat, Inc.
        // Copyright © 2018       Purism SPC
        //
        // Permission to use, copy, modify, distribute, and sell this
        // software and its documentation for any purpose is hereby granted
        // without fee, provided that the above copyright notice appear in
        // all copies and that both that copyright notice and this permission
        // notice appear in supporting documentation, and that the name of
        // the copyright holders not be used in advertising or publicity
        // pertaining to distribution of the software without specific,
        // written prior permission.  The copyright holders make no
        // representations about the suitability of this software for any
        // purpose.  It is provided "as is" without express or implied
        // warranty.
        //
        // THE COPYRIGHT HOLDERS DISCLAIM ALL WARRANTIES WITH REGARD TO THIS
        // SOFTWARE, INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
        // FITNESS, IN NO EVENT SHALL THE COPYRIGHT HOLDERS BE LIABLE FOR ANY
        // SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
        // WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
        // AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
        // ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
        // THIS SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static ZWP_TEXT_INPUT_V3_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_text_input_v3", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "enable", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "disable", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_surrounding_text", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_text_change_cause", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_content_type", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_cursor_rectangle", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "commit", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "enter", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "leave", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "preedit_string", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::Yes),wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "commit_string", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::Yes),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "delete_surrounding_text", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_text_input_v3_interface),
        };
        static zwp_text_input_v3_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_text_input_v3_requests_enable_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_text_input_v3_requests_disable_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_text_input_v3_requests_set_surrounding_text_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_text_input_v3_requests_set_text_change_cause_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_text_input_v3_requests_set_content_type_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_text_input_v3_requests_set_cursor_rectangle_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_text_input_v3_requests_commit_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_text_input_v3_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_text_input_v3_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"enable".as_ptr(), signature: c"".as_ptr(), types: zwp_text_input_v3_requests_enable_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"disable".as_ptr(), signature: c"".as_ptr(), types: zwp_text_input_v3_requests_disable_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_surrounding_text".as_ptr(), signature: c"sii".as_ptr(), types: zwp_text_input_v3_requests_set_surrounding_text_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_text_change_cause".as_ptr(), signature: c"u".as_ptr(), types: zwp_text_input_v3_requests_set_text_change_cause_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_content_type".as_ptr(), signature: c"uu".as_ptr(), types: zwp_text_input_v3_requests_set_content_type_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_cursor_rectangle".as_ptr(), signature: c"iiii".as_ptr(), types: zwp_text_input_v3_requests_set_cursor_rectangle_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"commit".as_ptr(), signature: c"".as_ptr(), types: zwp_text_input_v3_requests_commit_types.0.as_ptr() },
        ]);
        static zwp_text_input_v3_events_enter_types: InterfacePointers = InterfacePointers(&[&wl_surface_interface,]);
        static zwp_text_input_v3_events_leave_types: InterfacePointers = InterfacePointers(&[&wl_surface_interface,]);
        static zwp_text_input_v3_events_preedit_string_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_text_input_v3_events_commit_string_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_text_input_v3_events_delete_surrounding_text_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_text_input_v3_events_done_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_text_input_v3_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"enter".as_ptr(), signature: c"o".as_ptr(), types: zwp_text_input_v3_events_enter_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"leave".as_ptr(), signature: c"o".as_ptr(), types: zwp_text_input_v3_events_leave_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"preedit_string".as_ptr(), signature: c"?sii".as_ptr(), types: zwp_text_input_v3_events_preedit_string_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"commit_string".as_ptr(), signature: c"?s".as_ptr(), types: zwp_text_input_v3_events_commit_string_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"delete_surrounding_text".as_ptr(), signature: c"uu".as_ptr(), types: zwp_text_input_v3_events_delete_surrounding_text_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"u".as_ptr(), types: zwp_text_input_v3_events_done_types.0.as_ptr() },
        ]);
        pub static zwp_text_input_v3_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_text_input_v3".as_ptr(), version: 1, request_count: 8, requests: zwp_text_input_v3_requests.0.as_ptr(), event_count: 6, events: zwp_text_input_v3_events.0.as_ptr(),
        };
        pub static ZWP_TEXT_INPUT_MANAGER_V3_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_text_input_manager_v3", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_text_input", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_TEXT_INPUT_V3_INTERFACE), arg_interfaces: &[&WL_SEAT_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&zwp_text_input_manager_v3_interface),
        };
        static zwp_text_input_manager_v3_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_text_input_manager_v3_requests_get_text_input_types: InterfacePointers = InterfacePointers(&[&zwp_text_input_v3_interface,&wl_seat_interface,]);
        static zwp_text_input_manager_v3_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_text_input_manager_v3_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_text_input".as_ptr(), signature: c"no".as_ptr(), types: zwp_text_input_manager_v3_requests_get_text_input_types.0.as_ptr() },
        ]);
        pub static zwp_text_input_manager_v3_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_text_input_manager_v3".as_ptr(), version: 1, request_count: 2, requests: zwp_text_input_manager_v3_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };

            }
            use self::__interfaces::*;
            #[doc = "text input\n\nThe zwp_text_input_v3 interface represents text input and input methods\nassociated with a seat. It provides enter/leave events to follow the\ntext input focus for a seat.\n\nRequests are used to enable/disable the text-input object and set\nstate information like surrounding and selected text or the content type.\nThe information about the entered text is sent to the text-input object\nvia the preedit_string and commit_string events.\n\nText is valid UTF-8 encoded, indices and lengths are in bytes. Indices\nmust not point to middle bytes inside a code point: they must either\npoint to the first byte of a code point or to the end of the buffer.\nLengths must be measured between two valid indices.\n\nFocus moving throughout surfaces will result in the emission of\nzwp_text_input_v3.enter and zwp_text_input_v3.leave events. The focused\nsurface must commit zwp_text_input_v3.enable and\nzwp_text_input_v3.disable requests as the keyboard focus moves across\neditable and non-editable elements of the UI. Those two requests are not\nexpected to be paired with each other, the compositor must be able to\nhandle consecutive series of the same request.\n\nState is sent by the state requests (set_surrounding_text,\nset_content_type and set_cursor_rectangle) and a commit request. After an\nenter event or disable request all state information is invalidated and\nneeds to be resent by the client."]
        pub mod zwp_text_input_v3 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[doc = "text change reason\n\nReason for the change of surrounding text or cursor posision."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum ChangeCause {
        #[doc = "input method caused the change"]
            InputMethod = 0,
        #[doc = "something else than the input method caused the change"]
            Other = 1,
        }
        impl TryFrom<u32> for ChangeCause {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::InputMethod),
                1 => Ok(Self::Other),
                _ => Err(()),
            } }
        }
        impl From<ChangeCause> for u32 { fn from(value: ChangeCause) -> u32 { value as u32 } }
        bitflags::bitflags! {
            #[doc = "content hint\n\nContent hint is a bitmask to allow to modify the behavior of the text\ninput."]
            #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
            pub struct ContentHint: u32 {
                #[doc = "no special behavior"]
                const None = 0;
                #[doc = "suggest word completions"]
                const Completion = 1;
                #[doc = "suggest word corrections"]
                const Spellcheck = 2;
                #[doc = "switch to uppercase letters at the start of a sentence"]
                const AutoCapitalization = 4;
                #[doc = "prefer lowercase letters"]
                const Lowercase = 8;
                #[doc = "prefer uppercase letters"]
                const Uppercase = 16;
                #[doc = "prefer casing for titles and headings (can be language dependent)"]
                const Titlecase = 32;
                #[doc = "characters should be hidden"]
                const HiddenText = 64;
                #[doc = "typed text should not be stored"]
                const SensitiveData = 128;
                #[doc = "just Latin characters should be entered"]
                const Latin = 256;
                #[doc = "the text input is multiline"]
                const Multiline = 512;
            }
        }
        impl TryFrom<u32> for ContentHint {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { Self::from_bits(value).ok_or(()) }
        }
        impl From<ContentHint> for u32 { fn from(value: ContentHint) -> u32 { value.bits() } }
        #[doc = "content purpose\n\nThe content purpose allows to specify the primary purpose of a text\ninput.\n\nThis allows an input method to show special purpose input panels with\nextra characters or to disallow some characters."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum ContentPurpose {
        #[doc = "default input, allowing all characters"]
            Normal = 0,
        #[doc = "allow only alphabetic characters"]
            Alpha = 1,
        #[doc = "allow only digits"]
            Digits = 2,
        #[doc = "input a number (including decimal separator and sign)"]
            Number = 3,
        #[doc = "input a phone number"]
            Phone = 4,
        #[doc = "input an URL"]
            Url = 5,
        #[doc = "input an email address"]
            Email = 6,
        #[doc = "input a name of a person"]
            Name = 7,
        #[doc = "input a password (combine with sensitive_data hint)"]
            Password = 8,
        #[doc = "input is a numeric password (combine with sensitive_data hint)"]
            Pin = 9,
        #[doc = "input a date"]
            Date = 10,
        #[doc = "input a time"]
            Time = 11,
        #[doc = "input a date and time"]
            Datetime = 12,
        #[doc = "input for a terminal"]
            Terminal = 13,
        }
        impl TryFrom<u32> for ContentPurpose {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::Normal),
                1 => Ok(Self::Alpha),
                2 => Ok(Self::Digits),
                3 => Ok(Self::Number),
                4 => Ok(Self::Phone),
                5 => Ok(Self::Url),
                6 => Ok(Self::Email),
                7 => Ok(Self::Name),
                8 => Ok(Self::Password),
                9 => Ok(Self::Pin),
                10 => Ok(Self::Date),
                11 => Ok(Self::Time),
                12 => Ok(Self::Datetime),
                13 => Ok(Self::Terminal),
                _ => Err(()),
            } }
        }
        impl From<ContentPurpose> for u32 { fn from(value: ContentPurpose) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "Destroy the wp_text_input\n\nDestroy the wp_text_input object. Also disables all surfaces enabled\nthrough this wp_text_input object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "Request text input to be enabled\n\nRequests text input on the surface previously obtained from the enter\nevent.\n\nThis request must be issued every time the focused text input changes\nto a new one, including within the current surface. Use\nzwp_text_input_v3.disable when there is no longer any input focus on\nthe current surface.\n\nClients must not enable more than one text input on the single seat\nand should disable the current text input before enabling the new one.\nRequests to enable a text input when another text input is enabled\non the same seat must be ignored by compositor.\n\nThis request resets all state associated with previous enable, disable,\nset_surrounding_text, set_text_change_cause, set_content_type, and\nset_cursor_rectangle requests, as well as the state associated with\npreedit_string, commit_string, and delete_surrounding_text events.\n\nThe set_surrounding_text, set_content_type and set_cursor_rectangle\nrequests must follow if the text input supports the necessary\nfunctionality.\n\nState set with this request is double-buffered. It will get applied on\nthe next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe changes must be applied by the compositor after issuing a\nzwp_text_input_v3.commit request."]
            Enable,
        #[doc = "Disable text input on a surface\n\nExplicitly disable text input on the current surface (typically when\nthere is no focus on any text entry inside the surface).\n\nState set with this request is double-buffered. It will get applied on\nthe next zwp_text_input_v3.commit request."]
            Disable,
        #[doc = "sets the surrounding text\n\nSets the surrounding plain text around the input, excluding the preedit\ntext.\n\nThe client should notify the compositor of any changes in any of the\nvalues carried with this request, including changes caused by handling\nincoming text-input events as well as changes caused by other\nmechanisms like keyboard typing.\n\nIf the client is unaware of the text around the cursor, it should not\nissue this request, to signify lack of support to the compositor.\n\nText is UTF-8 encoded, and should include the cursor position, the\ncomplete selection and additional characters before and after them.\nThere is a maximum length of wayland messages, so text can not be\nlonger than 4000 bytes.\n\nCursor is the byte offset of the cursor within text buffer.\n\nAnchor is the byte offset of the selection anchor within text buffer.\nIf there is no selected text, anchor is the same as cursor.\n\nIf any preedit text is present, it is replaced with a cursor for the\npurpose of this event.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe initial state for affected fields is empty, meaning that the text\ninput does not support sending surrounding text. If the empty values\nget applied, subsequent attempts to change them may have no effect."]
            SetSurroundingText {
                text: String,
                cursor: i32,
                anchor: i32,
            },
        #[doc = "indicates the cause of surrounding text change\n\nTells the compositor why the text surrounding the cursor changed.\n\nWhenever the client detects an external change in text, cursor, or\nanchor posision, it must issue this request to the compositor. This\nrequest is intended to give the input method a chance to update the\npreedit text in an appropriate way, e.g. by removing it when the user\nstarts typing with a keyboard.\n\ncause describes the source of the change.\n\nThe value set with this request is double-buffered. It must be applied\nand reset to initial at the next zwp_text_input_v3.commit request.\n\nThe initial value of cause is input_method."]
            SetTextChangeCause {
                cause: WEnum<ChangeCause>,
            },
        #[doc = "set content purpose and hint\n\nSets the content purpose and content hint. While the purpose is the\nbasic purpose of an input field, the hint flags allow to modify some of\nthe behavior.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request.\nSubsequent attempts to update them may have no effect. The values\nremain valid until the next committed enable or disable request.\n\nThe initial value for hint is none, and the initial value for purpose\nis normal."]
            SetContentType {
                hint: WEnum<ContentHint>,
                purpose: WEnum<ContentPurpose>,
            },
        #[doc = "set cursor position\n\nMarks an area around the cursor as a x, y, width, height rectangle in\nsurface local coordinates.\n\nAllows the compositor to put a window with word suggestions near the\ncursor, without obstructing the text being input.\n\nIf the client is unaware of the position of edited text, it should not\nissue this request, to signify lack of support to the compositor.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe initial values describing a cursor rectangle are empty. That means\nthe text input does not support describing the cursor area. If the\nempty values get applied, subsequent attempts to change them may have\nno effect."]
            SetCursorRectangle {
                x: i32,
                y: i32,
                width: i32,
                height: i32,
            },
        #[doc = "commit state\n\nAtomically applies state changes recently sent to the compositor.\n\nThe commit request establishes and updates the state of the client, and\nmust be issued after any changes to apply them.\n\nText input state (enabled status, content purpose, content hint,\nsurrounding text and change cause, cursor rectangle) is conceptually\ndouble-buffered within the context of a text input, i.e. between a\ncommitted enable request and the following committed enable or disable\nrequest.\n\nProtocol requests modify the pending state, as opposed to the current\nstate in use by the input method. A commit request atomically applies\nall pending state, replacing the current state. After commit, the new\npending state is as documented for each related request.\n\nRequests are applied in the order of arrival.\n\nNeither current nor pending state are modified unless noted otherwise.\n\nThe compositor must count the number of commit requests coming from\neach zwp_text_input_v3 object and use the count as the serial in done\nevents."]
            Commit,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::Enable => 1,
            Self::Disable => 2,
            Self::SetSurroundingText { .. } => 3,
            Self::SetTextChangeCause { .. } => 4,
            Self::SetContentType { .. } => 5,
            Self::SetCursorRectangle { .. } => 6,
            Self::Commit => 7,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_ENABLE_SINCE: u32 = 1;
        pub const REQ_ENABLE_OPCODE: u16 = 1;
        pub const REQ_DISABLE_SINCE: u32 = 1;
        pub const REQ_DISABLE_OPCODE: u16 = 2;
        pub const REQ_SET_SURROUNDING_TEXT_SINCE: u32 = 1;
        pub const REQ_SET_SURROUNDING_TEXT_OPCODE: u16 = 3;
        pub const REQ_SET_TEXT_CHANGE_CAUSE_SINCE: u32 = 1;
        pub const REQ_SET_TEXT_CHANGE_CAUSE_OPCODE: u16 = 4;
        pub const REQ_SET_CONTENT_TYPE_SINCE: u32 = 1;
        pub const REQ_SET_CONTENT_TYPE_OPCODE: u16 = 5;
        pub const REQ_SET_CURSOR_RECTANGLE_SINCE: u32 = 1;
        pub const REQ_SET_CURSOR_RECTANGLE_OPCODE: u16 = 6;
        pub const REQ_COMMIT_SINCE: u32 = 1;
        pub const REQ_COMMIT_OPCODE: u16 = 7;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "enter event\n\nNotification that this seat's text-input focus is on a certain surface.\n\nIf client has created multiple text input objects, compositor must send\nthis event to all of them.\n\nWhen the seat has the keyboard capability the text-input focus follows\nthe keyboard focus. This event sets the current surface for the\ntext-input object."]
            Enter {
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "leave event\n\nNotification that this seat's text-input focus is no longer on a\ncertain surface. The client should reset any preedit string previously\nset.\n\nThe leave notification clears the current surface. It is sent before\nthe enter notification for the new focus. After leave event, compositor\nmust ignore requests from any text input instances until next enter\nevent.\n\nWhen the seat has the keyboard capability the text-input focus follows\nthe keyboard focus."]
            Leave {
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "pre-edit\n\nNotify when a new composing text (pre-edit) should be set at the\ncurrent cursor position. Any previously set composing text must be\nremoved. Any previously existing selected text must be removed.\n\nThe argument text contains the pre-edit string buffer.\n\nThe parameters cursor_begin and cursor_end are counted in bytes\nrelative to the beginning of the submitted text buffer. Cursor should\nbe hidden when both are equal to -1.\n\nThey could be represented by the client as a line if both values are\nthe same, or as a text highlight otherwise.\n\nValues set with this event are double-buffered. They must be applied\nand reset to initial on the next zwp_text_input_v3.done event.\n\nThe initial value of text is an empty string, and cursor_begin,\ncursor_end and cursor_hidden are all 0."]
            PreeditString {
                text: Option<String>,
                cursor_begin: i32,
                cursor_end: i32,
            },
        #[doc = "text commit\n\nNotify when text should be inserted into the editor widget. The text to\ncommit could be either just a single character after a key press or the\nresult of some composing (pre-edit).\n\nValues set with this event are double-buffered. They must be applied\nand reset to initial on the next zwp_text_input_v3.done event.\n\nThe initial value of text is an empty string."]
            CommitString {
                text: Option<String>,
            },
        #[doc = "delete surrounding text\n\nNotify when the text around the current cursor position should be\ndeleted.\n\nBefore_length and after_length are the number of bytes before and after\nthe current cursor index (excluding the selection) to delete.\n\nIf a preedit text is present, in effect before_length is counted from\nthe beginning of it, and after_length from its end (see done event\nsequence).\n\nValues set with this event are double-buffered. They must be applied\nand reset to initial on the next zwp_text_input_v3.done event.\n\nThe initial values of both before_length and after_length are 0."]
            DeleteSurroundingText {
        #[doc = "length of text before current cursor position"]
                before_length: u32,
        #[doc = "length of text after current cursor position"]
                after_length: u32,
            },
        #[doc = "apply changes\n\nInstruct the application to apply changes to state requested by the\npreedit_string, commit_string and delete_surrounding_text events. The\nstate relating to these events is double-buffered, and each one\nmodifies the pending state. This event replaces the current state with\nthe pending state.\n\nThe application must proceed by evaluating the changes in the following\norder:\n\n1. Replace existing preedit string with the cursor.\n2. Delete requested surrounding text.\n3. Insert commit string with the cursor at its end.\n4. Calculate surrounding text to send.\n5. Insert new preedit text in cursor position.\n6. Place cursor inside preedit text.\n\nThe serial number reflects the last state of the zwp_text_input_v3\nobject known to the compositor. The value of the serial argument must\nbe equal to the number of commit requests already issued on that object.\n\nWhen the client receives a done event with a serial different than the\nnumber of past commit requests, it must proceed with evaluating and\napplying the changes as normal, except it should not change the current\nstate of the zwp_text_input_v3 object. All pending state requests\n(set_surrounding_text, set_content_type and set_cursor_rectangle) on\nthe zwp_text_input_v3 object should be sent and committed after\nreceiving a zwp_text_input_v3.done event with a matching serial."]
            Done {
                serial: u32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Enter { .. } => 0,
            Self::Leave { .. } => 1,
            Self::PreeditString { .. } => 2,
            Self::CommitString { .. } => 3,
            Self::DeleteSurroundingText { .. } => 4,
            Self::Done { .. } => 5,
        } } }
        pub const EVT_ENTER_SINCE: u32 = 1;
        pub const EVT_ENTER_OPCODE: u16 = 0;
        pub const EVT_LEAVE_SINCE: u32 = 1;
        pub const EVT_LEAVE_OPCODE: u16 = 1;
        pub const EVT_PREEDIT_STRING_SINCE: u32 = 1;
        pub const EVT_PREEDIT_STRING_OPCODE: u16 = 2;
        pub const EVT_COMMIT_STRING_SINCE: u32 = 1;
        pub const EVT_COMMIT_STRING_OPCODE: u16 = 3;
        pub const EVT_DELETE_SURROUNDING_TEXT_SINCE: u32 = 1;
        pub const EVT_DELETE_SURROUNDING_TEXT_OPCODE: u16 = 4;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 5;

        #[derive(Debug, Clone)]
        pub struct ZwpTextInputV3 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTextInputV3 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTextInputV3 {}
        impl PartialEq<Weak<ZwpTextInputV3>> for ZwpTextInputV3 {
            fn eq(&self, other: &Weak<ZwpTextInputV3>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTextInputV3 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTextInputV3 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTextInputV3 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TEXT_INPUT_V3_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Object(surface)),) = (arg_iter.next(),) {
                        Ok((me, Event::Enter { surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Object(surface)),) = (arg_iter.next(),) {
                        Ok((me, Event::Leave { surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Str(text)), Some(Argument::Int(cursor_begin)), Some(Argument::Int(cursor_end)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::PreeditString { text: text.as_ref().map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned()), cursor_begin: cursor_begin, cursor_end: cursor_end }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                3 => {
                    if let (Some(Argument::Str(text)),) = (arg_iter.next(),) {
                        Ok((me, Event::CommitString { text: text.as_ref().map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned()) }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                4 => {
                    if let (Some(Argument::Uint(before_length)), Some(Argument::Uint(after_length)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::DeleteSurroundingText { before_length: before_length, after_length: after_length }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                5 => {
                    if let (Some(Argument::Uint(serial)),) = (arg_iter.next(),) {
                        Ok((me, Event::Done { serial: serial }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::Enable => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::Disable => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::SetSurroundingText { text, cursor, anchor } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(text).unwrap()))));
                    msg.args.push(Argument::Int(cursor));
                    msg.args.push(Argument::Int(anchor));
                    Ok((msg, child_spec))
                },
                Request::SetTextChangeCause { cause } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 4, args: Default::default() };
                    msg.args.push(Argument::Uint(u32::from(cause)));
                    Ok((msg, child_spec))
                },
                Request::SetContentType { hint, purpose } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 5, args: Default::default() };
                    msg.args.push(Argument::Uint(u32::from(hint)));
                    msg.args.push(Argument::Uint(u32::from(purpose)));
                    Ok((msg, child_spec))
                },
                Request::SetCursorRectangle { x, y, width, height } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 6, args: Default::default() };
                    msg.args.push(Argument::Int(x));
                    msg.args.push(Argument::Int(y));
                    msg.args.push(Argument::Int(width));
                    msg.args.push(Argument::Int(height));
                    Ok((msg, child_spec))
                },
                Request::Commit => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 7, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTextInputV3 {
        #[doc = "Destroy the wp_text_input\n\nDestroy the wp_text_input object. Also disables all surfaces enabled\nthrough this wp_text_input object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "Request text input to be enabled\n\nRequests text input on the surface previously obtained from the enter\nevent.\n\nThis request must be issued every time the focused text input changes\nto a new one, including within the current surface. Use\nzwp_text_input_v3.disable when there is no longer any input focus on\nthe current surface.\n\nClients must not enable more than one text input on the single seat\nand should disable the current text input before enabling the new one.\nRequests to enable a text input when another text input is enabled\non the same seat must be ignored by compositor.\n\nThis request resets all state associated with previous enable, disable,\nset_surrounding_text, set_text_change_cause, set_content_type, and\nset_cursor_rectangle requests, as well as the state associated with\npreedit_string, commit_string, and delete_surrounding_text events.\n\nThe set_surrounding_text, set_content_type and set_cursor_rectangle\nrequests must follow if the text input supports the necessary\nfunctionality.\n\nState set with this request is double-buffered. It will get applied on\nthe next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe changes must be applied by the compositor after issuing a\nzwp_text_input_v3.commit request."]
        pub fn enable(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Enable, None);
        }
        #[doc = "Disable text input on a surface\n\nExplicitly disable text input on the current surface (typically when\nthere is no focus on any text entry inside the surface).\n\nState set with this request is double-buffered. It will get applied on\nthe next zwp_text_input_v3.commit request."]
        pub fn disable(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Disable, None);
        }
        #[doc = "sets the surrounding text\n\nSets the surrounding plain text around the input, excluding the preedit\ntext.\n\nThe client should notify the compositor of any changes in any of the\nvalues carried with this request, including changes caused by handling\nincoming text-input events as well as changes caused by other\nmechanisms like keyboard typing.\n\nIf the client is unaware of the text around the cursor, it should not\nissue this request, to signify lack of support to the compositor.\n\nText is UTF-8 encoded, and should include the cursor position, the\ncomplete selection and additional characters before and after them.\nThere is a maximum length of wayland messages, so text can not be\nlonger than 4000 bytes.\n\nCursor is the byte offset of the cursor within text buffer.\n\nAnchor is the byte offset of the selection anchor within text buffer.\nIf there is no selected text, anchor is the same as cursor.\n\nIf any preedit text is present, it is replaced with a cursor for the\npurpose of this event.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe initial state for affected fields is empty, meaning that the text\ninput does not support sending surrounding text. If the empty values\nget applied, subsequent attempts to change them may have no effect."]
        pub fn set_surrounding_text(&self, text: String, cursor: i32, anchor: i32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetSurroundingText { text: text, cursor: cursor, anchor: anchor }, None);
        }
        #[doc = "indicates the cause of surrounding text change\n\nTells the compositor why the text surrounding the cursor changed.\n\nWhenever the client detects an external change in text, cursor, or\nanchor posision, it must issue this request to the compositor. This\nrequest is intended to give the input method a chance to update the\npreedit text in an appropriate way, e.g. by removing it when the user\nstarts typing with a keyboard.\n\ncause describes the source of the change.\n\nThe value set with this request is double-buffered. It must be applied\nand reset to initial at the next zwp_text_input_v3.commit request.\n\nThe initial value of cause is input_method."]
        pub fn set_text_change_cause(&self, cause: ChangeCause) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetTextChangeCause { cause: WEnum::Value(cause) }, None);
        }
        #[doc = "set content purpose and hint\n\nSets the content purpose and content hint. While the purpose is the\nbasic purpose of an input field, the hint flags allow to modify some of\nthe behavior.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request.\nSubsequent attempts to update them may have no effect. The values\nremain valid until the next committed enable or disable request.\n\nThe initial value for hint is none, and the initial value for purpose\nis normal."]
        pub fn set_content_type(&self, hint: ContentHint, purpose: ContentPurpose) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetContentType { hint: WEnum::Value(hint), purpose: WEnum::Value(purpose) }, None);
        }
        #[doc = "set cursor position\n\nMarks an area around the cursor as a x, y, width, height rectangle in\nsurface local coordinates.\n\nAllows the compositor to put a window with word suggestions near the\ncursor, without obstructing the text being input.\n\nIf the client is unaware of the position of edited text, it should not\nissue this request, to signify lack of support to the compositor.\n\nValues set with this request are double-buffered. They will get applied\non the next zwp_text_input_v3.commit request, and stay valid until the\nnext committed enable or disable request.\n\nThe initial values describing a cursor rectangle are empty. That means\nthe text input does not support describing the cursor area. If the\nempty values get applied, subsequent attempts to change them may have\nno effect."]
        pub fn set_cursor_rectangle(&self, x: i32, y: i32, width: i32, height: i32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetCursorRectangle { x: x, y: y, width: width, height: height }, None);
        }
        #[doc = "commit state\n\nAtomically applies state changes recently sent to the compositor.\n\nThe commit request establishes and updates the state of the client, and\nmust be issued after any changes to apply them.\n\nText input state (enabled status, content purpose, content hint,\nsurrounding text and change cause, cursor rectangle) is conceptually\ndouble-buffered within the context of a text input, i.e. between a\ncommitted enable request and the following committed enable or disable\nrequest.\n\nProtocol requests modify the pending state, as opposed to the current\nstate in use by the input method. A commit request atomically applies\nall pending state, replacing the current state. After commit, the new\npending state is as documented for each related request.\n\nRequests are applied in the order of arrival.\n\nNeither current nor pending state are modified unless noted otherwise.\n\nThe compositor must count the number of commit requests coming from\neach zwp_text_input_v3 object and use the count as the serial in done\nevents."]
        pub fn commit(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Commit, None);
        }
        }
        }
        #[doc = "text input manager\n\nA factory for text-input objects. This object is a global singleton."]
        pub mod zwp_text_input_manager_v3 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "Destroy the wp_text_input_manager\n\nDestroy the wp_text_input_manager object."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "create a new text input object\n\nCreates a new text-input object for a given seat."]
            GetTextInput {
                seat: super::wl_seat::WlSeat,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::GetTextInput { .. } => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_GET_TEXT_INPUT_SINCE: u32 = 1;
        pub const REQ_GET_TEXT_INPUT_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct ZwpTextInputManagerV3 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpTextInputManagerV3 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpTextInputManagerV3 {}
        impl PartialEq<Weak<ZwpTextInputManagerV3>> for ZwpTextInputManagerV3 {
            fn eq(&self, other: &Weak<ZwpTextInputManagerV3>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpTextInputManagerV3 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpTextInputManagerV3 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpTextInputManagerV3 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_TEXT_INPUT_MANAGER_V3_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::GetTextInput { seat } => {
                    let child_spec = Some((super::zwp_text_input_v3::ZwpTextInputV3::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(seat.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpTextInputManagerV3 {
        #[doc = "Destroy the wp_text_input_manager\n\nDestroy the wp_text_input_manager object."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "create a new text input object\n\nCreates a new text-input object for a given seat."]
        pub fn get_text_input<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_text_input_v3::ZwpTextInputV3, U> + 'static>(&self, seat: &super::wl_seat::WlSeat, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_text_input_v3::ZwpTextInputV3 {
            self.send_constructor::<super::zwp_text_input_v3::ZwpTextInputV3>(Request::GetTextInput { seat: seat.clone() }, qh.make_data::<super::zwp_text_input_v3::ZwpTextInputV3, U>(udata)).unwrap_or_else(|_| super::zwp_text_input_v3::ZwpTextInputV3::inert(self.backend.clone()))
        }
        }
        }

        }
    }
}

#[cfg(feature = "unstable")]
pub mod pointer_gestures {
    pub mod zv1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2015, 2016 Red Hat
        //
        // Permission is hereby granted, free of charge, to any person obtaining a
        // copy of this software and associated documentation files (the "Software"),
        // to deal in the Software without restriction, including without limitation
        // the rights to use, copy, modify, merge, publish, distribute, sublicense,
        // and/or sell copies of the Software, and to permit persons to whom the
        // Software is furnished to do so, subject to the following conditions:
        //
        // The above copyright notice and this permission notice (including the next
        // paragraph) shall be included in all copies or substantial portions of the
        // Software.
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
        // THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
        // FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
        // DEALINGS IN THE SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static ZWP_POINTER_GESTURES_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_pointer_gestures_v1", version: 3,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "get_swipe_gesture", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_POINTER_GESTURE_SWIPE_V1_INTERFACE), arg_interfaces: &[&WL_POINTER_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "get_pinch_gesture", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_POINTER_GESTURE_PINCH_V1_INTERFACE), arg_interfaces: &[&WL_POINTER_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "release", since: 2, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_hold_gesture", since: 3, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_POINTER_GESTURE_HOLD_V1_INTERFACE), arg_interfaces: &[&WL_POINTER_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&zwp_pointer_gestures_v1_interface),
        };
        static zwp_pointer_gestures_v1_requests_get_swipe_gesture_types: InterfacePointers = InterfacePointers(&[&zwp_pointer_gesture_swipe_v1_interface,&wl_pointer_interface,]);
        static zwp_pointer_gestures_v1_requests_get_pinch_gesture_types: InterfacePointers = InterfacePointers(&[&zwp_pointer_gesture_pinch_v1_interface,&wl_pointer_interface,]);
        static zwp_pointer_gestures_v1_requests_release_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_pointer_gestures_v1_requests_get_hold_gesture_types: InterfacePointers = InterfacePointers(&[&zwp_pointer_gesture_hold_v1_interface,&wl_pointer_interface,]);
        static zwp_pointer_gestures_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"get_swipe_gesture".as_ptr(), signature: c"no".as_ptr(), types: zwp_pointer_gestures_v1_requests_get_swipe_gesture_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_pinch_gesture".as_ptr(), signature: c"no".as_ptr(), types: zwp_pointer_gestures_v1_requests_get_pinch_gesture_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"release".as_ptr(), signature: c"2".as_ptr(), types: zwp_pointer_gestures_v1_requests_release_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_hold_gesture".as_ptr(), signature: c"3no".as_ptr(), types: zwp_pointer_gestures_v1_requests_get_hold_gesture_types.0.as_ptr() },
        ]);
        pub static zwp_pointer_gestures_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_pointer_gestures_v1".as_ptr(), version: 3, request_count: 4, requests: zwp_pointer_gestures_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static ZWP_POINTER_GESTURE_SWIPE_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_pointer_gesture_swipe_v1", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "begin", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "update", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "end", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_pointer_gesture_swipe_v1_interface),
        };
        static zwp_pointer_gesture_swipe_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_pointer_gesture_swipe_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_pointer_gesture_swipe_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_pointer_gesture_swipe_v1_events_begin_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),&wl_surface_interface,core::ptr::null(),]);
        static zwp_pointer_gesture_swipe_v1_events_update_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_pointer_gesture_swipe_v1_events_end_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_pointer_gesture_swipe_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"begin".as_ptr(), signature: c"uuou".as_ptr(), types: zwp_pointer_gesture_swipe_v1_events_begin_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"update".as_ptr(), signature: c"uff".as_ptr(), types: zwp_pointer_gesture_swipe_v1_events_update_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"end".as_ptr(), signature: c"uui".as_ptr(), types: zwp_pointer_gesture_swipe_v1_events_end_types.0.as_ptr() },
        ]);
        pub static zwp_pointer_gesture_swipe_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_pointer_gesture_swipe_v1".as_ptr(), version: 2, request_count: 1, requests: zwp_pointer_gesture_swipe_v1_requests.0.as_ptr(), event_count: 3, events: zwp_pointer_gesture_swipe_v1_events.0.as_ptr(),
        };
        pub static ZWP_POINTER_GESTURE_PINCH_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_pointer_gesture_pinch_v1", version: 2,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "begin", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "update", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "end", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_pointer_gesture_pinch_v1_interface),
        };
        static zwp_pointer_gesture_pinch_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_pointer_gesture_pinch_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_pointer_gesture_pinch_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_pointer_gesture_pinch_v1_events_begin_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),&wl_surface_interface,core::ptr::null(),]);
        static zwp_pointer_gesture_pinch_v1_events_update_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_pointer_gesture_pinch_v1_events_end_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_pointer_gesture_pinch_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"begin".as_ptr(), signature: c"uuou".as_ptr(), types: zwp_pointer_gesture_pinch_v1_events_begin_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"update".as_ptr(), signature: c"uffff".as_ptr(), types: zwp_pointer_gesture_pinch_v1_events_update_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"end".as_ptr(), signature: c"uui".as_ptr(), types: zwp_pointer_gesture_pinch_v1_events_end_types.0.as_ptr() },
        ]);
        pub static zwp_pointer_gesture_pinch_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_pointer_gesture_pinch_v1".as_ptr(), version: 2, request_count: 1, requests: zwp_pointer_gesture_pinch_v1_requests.0.as_ptr(), event_count: 3, events: zwp_pointer_gesture_pinch_v1_events.0.as_ptr(),
        };
        pub static ZWP_POINTER_GESTURE_HOLD_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_pointer_gesture_hold_v1", version: 3,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 3, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "begin", since: 3, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "end", since: 3, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_pointer_gesture_hold_v1_interface),
        };
        static zwp_pointer_gesture_hold_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_pointer_gesture_hold_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"3".as_ptr(), types: zwp_pointer_gesture_hold_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_pointer_gesture_hold_v1_events_begin_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),&wl_surface_interface,core::ptr::null(),]);
        static zwp_pointer_gesture_hold_v1_events_end_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
        static zwp_pointer_gesture_hold_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"begin".as_ptr(), signature: c"3uuou".as_ptr(), types: zwp_pointer_gesture_hold_v1_events_begin_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"end".as_ptr(), signature: c"3uui".as_ptr(), types: zwp_pointer_gesture_hold_v1_events_end_types.0.as_ptr() },
        ]);
        pub static zwp_pointer_gesture_hold_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_pointer_gesture_hold_v1".as_ptr(), version: 3, request_count: 1, requests: zwp_pointer_gesture_hold_v1_requests.0.as_ptr(), event_count: 2, events: zwp_pointer_gesture_hold_v1_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "touchpad gestures\n\nA global interface to provide semantic touchpad gestures for a given\npointer.\n\nThree gestures are currently supported: swipe, pinch, and hold.\nPinch and swipe gestures follow a three-stage cycle: begin, update,\nend, hold gestures follow a two-stage cycle: begin and end. All\ngestures are identified by a unique id.\n\nWarning! The protocol described in this file is experimental and\nbackward incompatible changes may be made. Backward compatible changes\nmay be added together with the corresponding interface version bump.\nBackward incompatible changes are done by bumping the version number in\nthe protocol and interface names and resetting the interface version.\nOnce the protocol is to be declared stable, the 'z' prefix and the\nversion number in the protocol and interface names are removed and the\ninterface version number is reset."]
        pub mod zwp_pointer_gestures_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "get swipe gesture\n\nCreate a swipe gesture object. See the\nwl_pointer_gesture_swipe interface for details."]
            GetSwipeGesture {
                pointer: super::wl_pointer::WlPointer,
            },
        #[doc = "get pinch gesture\n\nCreate a pinch gesture object. See the\nwl_pointer_gesture_pinch interface for details."]
            GetPinchGesture {
                pointer: super::wl_pointer::WlPointer,
            },
        #[doc = "destroy the pointer gesture object\n\nDestroy the pointer gesture object. Swipe, pinch and hold objects\ncreated via this gesture object remain valid."]
        #[doc = "Available since interface version 2."]
        #[doc = "This message destroys the protocol object."]
            Release,
        #[doc = "get hold gesture\n\nCreate a hold gesture object. See the\nwl_pointer_gesture_hold interface for details."]
        #[doc = "Available since interface version 3."]
            GetHoldGesture {
                pointer: super::wl_pointer::WlPointer,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::GetSwipeGesture { .. } => 0,
            Self::GetPinchGesture { .. } => 1,
            Self::Release => 2,
            Self::GetHoldGesture { .. } => 3,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_GET_SWIPE_GESTURE_SINCE: u32 = 1;
        pub const REQ_GET_SWIPE_GESTURE_OPCODE: u16 = 0;
        pub const REQ_GET_PINCH_GESTURE_SINCE: u32 = 1;
        pub const REQ_GET_PINCH_GESTURE_OPCODE: u16 = 1;
        pub const REQ_RELEASE_SINCE: u32 = 2;
        pub const REQ_RELEASE_OPCODE: u16 = 2;
        pub const REQ_GET_HOLD_GESTURE_SINCE: u32 = 3;
        pub const REQ_GET_HOLD_GESTURE_OPCODE: u16 = 3;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct ZwpPointerGesturesV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPointerGesturesV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPointerGesturesV1 {}
        impl PartialEq<Weak<ZwpPointerGesturesV1>> for ZwpPointerGesturesV1 {
            fn eq(&self, other: &Weak<ZwpPointerGesturesV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPointerGesturesV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPointerGesturesV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPointerGesturesV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_POINTER_GESTURES_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::GetSwipeGesture { pointer } => {
                    let child_spec = Some((super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(pointer.id()));
                    Ok((msg, child_spec))
                },
                Request::GetPinchGesture { pointer } => {
                    let child_spec = Some((super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(pointer.id()));
                    Ok((msg, child_spec))
                },
                Request::Release => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::GetHoldGesture { pointer } => {
                    let child_spec = Some((super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(pointer.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPointerGesturesV1 {
        #[doc = "get swipe gesture\n\nCreate a swipe gesture object. See the\nwl_pointer_gesture_swipe interface for details."]
        pub fn get_swipe_gesture<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1, U> + 'static>(&self, pointer: &super::wl_pointer::WlPointer, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1 {
            self.send_constructor::<super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1>(Request::GetSwipeGesture { pointer: pointer.clone() }, qh.make_data::<super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1, U>(udata)).unwrap_or_else(|_| super::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1::inert(self.backend.clone()))
        }
        #[doc = "get pinch gesture\n\nCreate a pinch gesture object. See the\nwl_pointer_gesture_pinch interface for details."]
        pub fn get_pinch_gesture<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, U> + 'static>(&self, pointer: &super::wl_pointer::WlPointer, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1 {
            self.send_constructor::<super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1>(Request::GetPinchGesture { pointer: pointer.clone() }, qh.make_data::<super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, U>(udata)).unwrap_or_else(|_| super::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1::inert(self.backend.clone()))
        }
        #[doc = "destroy the pointer gesture object\n\nDestroy the pointer gesture object. Swipe, pinch and hold objects\ncreated via this gesture object remain valid."]
        pub fn release(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Release, None);
        }
        #[doc = "get hold gesture\n\nCreate a hold gesture object. See the\nwl_pointer_gesture_hold interface for details."]
        pub fn get_hold_gesture<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1, U> + 'static>(&self, pointer: &super::wl_pointer::WlPointer, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1 {
            self.send_constructor::<super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1>(Request::GetHoldGesture { pointer: pointer.clone() }, qh.make_data::<super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1, U>(udata)).unwrap_or_else(|_| super::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1::inert(self.backend.clone()))
        }
        }
        }
        #[doc = "a swipe gesture object\n\nA swipe gesture object notifies a client about a multi-finger swipe\ngesture detected on an indirect input device such as a touchpad.\nThe gesture is usually initiated by multiple fingers moving in the\nsame direction but once initiated the direction may change.\nThe precise conditions of when such a gesture is detected are\nimplementation-dependent.\n\nA gesture consists of three stages: begin, update (optional) and end.\nThere cannot be multiple simultaneous hold, pinch or swipe gestures on a\nsame pointer/seat, how compositors prevent these situations is\nimplementation-dependent.\n\nA gesture may be cancelled by the compositor or the hardware.\nClients should not consider performing permanent or irreversible\nactions until the end of a gesture has been received."]
        pub mod zwp_pointer_gesture_swipe_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the pointer swipe gesture object\n\n"]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "multi-finger swipe begin\n\nThis event is sent when a multi-finger swipe gesture is detected\non the device."]
            Begin {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
                surface: super::wl_surface::WlSurface,
        #[doc = "number of fingers"]
                fingers: u32,
            },
        #[doc = "multi-finger swipe motion\n\nThis event is sent when a multi-finger swipe gesture changes the\nposition of the logical center.\n\nThe dx and dy coordinates are relative coordinates of the logical\ncenter of the gesture compared to the previous event."]
            Update {
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
        #[doc = "delta x coordinate in surface coordinate space"]
                dx: f64,
        #[doc = "delta y coordinate in surface coordinate space"]
                dy: f64,
            },
        #[doc = "multi-finger swipe end\n\nThis event is sent when a multi-finger swipe gesture ceases to\nbe valid. This may happen when one or more fingers are lifted or\nthe gesture is cancelled.\n\nWhen a gesture is cancelled, the client should undo state changes\ncaused by this gesture. What causes a gesture to be cancelled is\nimplementation-dependent."]
            End {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
        #[doc = "1 if the gesture was cancelled, 0 otherwise"]
                cancelled: i32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Begin { .. } => 0,
            Self::Update { .. } => 1,
            Self::End { .. } => 2,
        } } }
        pub const EVT_BEGIN_SINCE: u32 = 1;
        pub const EVT_BEGIN_OPCODE: u16 = 0;
        pub const EVT_UPDATE_SINCE: u32 = 1;
        pub const EVT_UPDATE_OPCODE: u16 = 1;
        pub const EVT_END_SINCE: u32 = 1;
        pub const EVT_END_OPCODE: u16 = 2;

        #[derive(Debug, Clone)]
        pub struct ZwpPointerGestureSwipeV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPointerGestureSwipeV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPointerGestureSwipeV1 {}
        impl PartialEq<Weak<ZwpPointerGestureSwipeV1>> for ZwpPointerGestureSwipeV1 {
            fn eq(&self, other: &Weak<ZwpPointerGestureSwipeV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPointerGestureSwipeV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPointerGestureSwipeV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPointerGestureSwipeV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_POINTER_GESTURE_SWIPE_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Object(surface)), Some(Argument::Uint(fingers)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Begin { serial: serial, time: time, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?, fingers: fingers }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(time)), Some(Argument::Fixed(dx)), Some(Argument::Fixed(dy)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Update { time: time, dx: (dx as f64) / 256.0, dy: (dy as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Int(cancelled)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::End { serial: serial, time: time, cancelled: cancelled }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPointerGestureSwipeV1 {
        #[doc = "destroy the pointer swipe gesture object\n\n"]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "a pinch gesture object\n\nA pinch gesture object notifies a client about a multi-finger pinch\ngesture detected on an indirect input device such as a touchpad.\nThe gesture is usually initiated by multiple fingers moving towards\neach other or away from each other, or by two or more fingers rotating\naround a logical center of gravity. The precise conditions of when\nsuch a gesture is detected are implementation-dependent.\n\nA gesture consists of three stages: begin, update (optional) and end.\nThere cannot be multiple simultaneous hold, pinch or swipe gestures on a\nsame pointer/seat, how compositors prevent these situations is\nimplementation-dependent.\n\nA gesture may be cancelled by the compositor or the hardware.\nClients should not consider performing permanent or irreversible\nactions until the end of a gesture has been received."]
        pub mod zwp_pointer_gesture_pinch_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the pinch gesture object\n\n"]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "multi-finger pinch begin\n\nThis event is sent when a multi-finger pinch gesture is detected\non the device."]
            Begin {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
                surface: super::wl_surface::WlSurface,
        #[doc = "number of fingers"]
                fingers: u32,
            },
        #[doc = "multi-finger pinch motion\n\nThis event is sent when a multi-finger pinch gesture changes the\nposition of the logical center, the rotation or the relative scale.\n\nThe dx and dy coordinates are relative coordinates in the\nsurface coordinate space of the logical center of the gesture.\n\nThe scale factor is an absolute scale compared to the pointer_gesture_pinch.begin\nevent, e.g. a scale of 2 means the fingers are now twice as far apart\nas on pointer_gesture_pinch.begin.\n\nThe rotation is the relative angle in degrees clockwise compared to the previous\npointer_gesture_pinch.begin or pointer_gesture_pinch.update event."]
            Update {
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
        #[doc = "delta x coordinate in surface coordinate space"]
                dx: f64,
        #[doc = "delta y coordinate in surface coordinate space"]
                dy: f64,
        #[doc = "scale relative to the initial finger position"]
                scale: f64,
        #[doc = "angle in degrees cw relative to the previous event"]
                rotation: f64,
            },
        #[doc = "multi-finger pinch end\n\nThis event is sent when a multi-finger pinch gesture ceases to\nbe valid. This may happen when one or more fingers are lifted or\nthe gesture is cancelled.\n\nWhen a gesture is cancelled, the client should undo state changes\ncaused by this gesture. What causes a gesture to be cancelled is\nimplementation-dependent."]
            End {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
        #[doc = "1 if the gesture was cancelled, 0 otherwise"]
                cancelled: i32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Begin { .. } => 0,
            Self::Update { .. } => 1,
            Self::End { .. } => 2,
        } } }
        pub const EVT_BEGIN_SINCE: u32 = 1;
        pub const EVT_BEGIN_OPCODE: u16 = 0;
        pub const EVT_UPDATE_SINCE: u32 = 1;
        pub const EVT_UPDATE_OPCODE: u16 = 1;
        pub const EVT_END_SINCE: u32 = 1;
        pub const EVT_END_OPCODE: u16 = 2;

        #[derive(Debug, Clone)]
        pub struct ZwpPointerGesturePinchV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPointerGesturePinchV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPointerGesturePinchV1 {}
        impl PartialEq<Weak<ZwpPointerGesturePinchV1>> for ZwpPointerGesturePinchV1 {
            fn eq(&self, other: &Weak<ZwpPointerGesturePinchV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPointerGesturePinchV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPointerGesturePinchV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPointerGesturePinchV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_POINTER_GESTURE_PINCH_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Object(surface)), Some(Argument::Uint(fingers)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Begin { serial: serial, time: time, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?, fingers: fingers }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(time)), Some(Argument::Fixed(dx)), Some(Argument::Fixed(dy)), Some(Argument::Fixed(scale)), Some(Argument::Fixed(rotation)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Update { time: time, dx: (dx as f64) / 256.0, dy: (dy as f64) / 256.0, scale: (scale as f64) / 256.0, rotation: (rotation as f64) / 256.0 }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                2 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Int(cancelled)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::End { serial: serial, time: time, cancelled: cancelled }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPointerGesturePinchV1 {
        #[doc = "destroy the pinch gesture object\n\n"]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "a hold gesture object\n\nA hold gesture object notifies a client about a single- or\nmulti-finger hold gesture detected on an indirect input device such as\na touchpad. The gesture is usually initiated by one or more fingers\nbeing held down without significant movement. The precise conditions\nof when such a gesture is detected are implementation-dependent.\n\nIn particular, this gesture may be used to cancel kinetic scrolling.\n\nA hold gesture consists of two stages: begin and end. Unlike pinch and\nswipe there is no update stage.\nThere cannot be multiple simultaneous hold, pinch or swipe gestures on a\nsame pointer/seat, how compositors prevent these situations is\nimplementation-dependent.\n\nA gesture may be cancelled by the compositor or the hardware.\nClients should not consider performing permanent or irreversible\nactions until the end of a gesture has been received."]
        pub mod zwp_pointer_gesture_hold_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the hold gesture object\n\n"]
        #[doc = "Available since interface version 3."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 3;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "multi-finger hold begin\n\nThis event is sent when a hold gesture is detected on the device."]
        #[doc = "Available since interface version 3."]
            Begin {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
                surface: super::wl_surface::WlSurface,
        #[doc = "number of fingers"]
                fingers: u32,
            },
        #[doc = "multi-finger hold end\n\nThis event is sent when a hold gesture ceases to\nbe valid. This may happen when the holding fingers are lifted or\nthe gesture is cancelled, for example if the fingers move past an\nimplementation-defined threshold, the finger count changes or the hold\ngesture changes into a different type of gesture.\n\nWhen a gesture is cancelled, the client may need to undo state changes\ncaused by this gesture. What causes a gesture to be cancelled is\nimplementation-dependent."]
        #[doc = "Available since interface version 3."]
            End {
                serial: u32,
        #[doc = "timestamp with millisecond granularity"]
                time: u32,
        #[doc = "1 if the gesture was cancelled, 0 otherwise"]
                cancelled: i32,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Begin { .. } => 0,
            Self::End { .. } => 1,
        } } }
        pub const EVT_BEGIN_SINCE: u32 = 3;
        pub const EVT_BEGIN_OPCODE: u16 = 0;
        pub const EVT_END_SINCE: u32 = 3;
        pub const EVT_END_OPCODE: u16 = 1;

        #[derive(Debug, Clone)]
        pub struct ZwpPointerGestureHoldV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPointerGestureHoldV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPointerGestureHoldV1 {}
        impl PartialEq<Weak<ZwpPointerGestureHoldV1>> for ZwpPointerGestureHoldV1 {
            fn eq(&self, other: &Weak<ZwpPointerGestureHoldV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPointerGestureHoldV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPointerGestureHoldV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPointerGestureHoldV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_POINTER_GESTURE_HOLD_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Object(surface)), Some(Argument::Uint(fingers)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Begin { serial: serial, time: time, surface: super::wl_surface::WlSurface::from_id(conn, surface.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?, fingers: fingers }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Uint(serial)), Some(Argument::Uint(time)), Some(Argument::Int(cancelled)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::End { serial: serial, time: time, cancelled: cancelled }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPointerGestureHoldV1 {
        #[doc = "destroy the hold gesture object\n\n"]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }

        }
    }
}

#[cfg(feature = "unstable")]
pub mod primary_selection {
    pub mod zv1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2015, 2016 Red Hat
        //
        // Permission is hereby granted, free of charge, to any person obtaining a
        // copy of this software and associated documentation files (the "Software"),
        // to deal in the Software without restriction, including without limitation
        // the rights to use, copy, modify, merge, publish, distribute, sublicense,
        // and/or sell copies of the Software, and to permit persons to whom the
        // Software is furnished to do so, subject to the following conditions:
        //
        // The above copyright notice and this permission notice (including the next
        // paragraph) shall be included in all copies or substantial portions of the
        // Software.
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
        // THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
        // FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
        // DEALINGS IN THE SOFTWARE.
            //! Client-side API of this protocol
            use wayland_client;
            use wayland_client::protocol::*;
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_primary_selection_device_manager_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "create_source", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_PRIMARY_SELECTION_SOURCE_V1_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_device", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZWP_PRIMARY_SELECTION_DEVICE_V1_INTERFACE), arg_interfaces: &[&WL_SEAT_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&zwp_primary_selection_device_manager_v1_interface),
        };
        static zwp_primary_selection_device_manager_v1_requests_create_source_types: InterfacePointers = InterfacePointers(&[&zwp_primary_selection_source_v1_interface,]);
        static zwp_primary_selection_device_manager_v1_requests_get_device_types: InterfacePointers = InterfacePointers(&[&zwp_primary_selection_device_v1_interface,&wl_seat_interface,]);
        static zwp_primary_selection_device_manager_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_primary_selection_device_manager_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"create_source".as_ptr(), signature: c"n".as_ptr(), types: zwp_primary_selection_device_manager_v1_requests_create_source_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_device".as_ptr(), signature: c"no".as_ptr(), types: zwp_primary_selection_device_manager_v1_requests_get_device_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_primary_selection_device_manager_v1_requests_destroy_types.0.as_ptr() },
        ]);
        pub static zwp_primary_selection_device_manager_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_primary_selection_device_manager_v1".as_ptr(), version: 1, request_count: 3, requests: zwp_primary_selection_device_manager_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static ZWP_PRIMARY_SELECTION_DEVICE_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_primary_selection_device_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_selection", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[&ZWP_PRIMARY_SELECTION_SOURCE_V1_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "data_offer", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&ZWP_PRIMARY_SELECTION_OFFER_V1_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "selection", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),],
                    child_interface: None, arg_interfaces: &[&ZWP_PRIMARY_SELECTION_OFFER_V1_INTERFACE,],
                },
            ],
            c_ptr: Some(&zwp_primary_selection_device_v1_interface),
        };
        static zwp_primary_selection_device_v1_requests_set_selection_types: InterfacePointers = InterfacePointers(&[&zwp_primary_selection_source_v1_interface,core::ptr::null(),]);
        static zwp_primary_selection_device_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_primary_selection_device_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_selection".as_ptr(), signature: c"?ou".as_ptr(), types: zwp_primary_selection_device_v1_requests_set_selection_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_primary_selection_device_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_primary_selection_device_v1_events_data_offer_types: InterfacePointers = InterfacePointers(&[&zwp_primary_selection_offer_v1_interface,]);
        static zwp_primary_selection_device_v1_events_selection_types: InterfacePointers = InterfacePointers(&[&zwp_primary_selection_offer_v1_interface,]);
        static zwp_primary_selection_device_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"data_offer".as_ptr(), signature: c"n".as_ptr(), types: zwp_primary_selection_device_v1_events_data_offer_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"selection".as_ptr(), signature: c"?o".as_ptr(), types: zwp_primary_selection_device_v1_events_selection_types.0.as_ptr() },
        ]);
        pub static zwp_primary_selection_device_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_primary_selection_device_v1".as_ptr(), version: 1, request_count: 2, requests: zwp_primary_selection_device_v1_requests.0.as_ptr(), event_count: 2, events: zwp_primary_selection_device_v1_events.0.as_ptr(),
        };
        pub static ZWP_PRIMARY_SELECTION_OFFER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_primary_selection_offer_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "receive", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Fd,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "offer", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_primary_selection_offer_v1_interface),
        };
        static zwp_primary_selection_offer_v1_requests_receive_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_primary_selection_offer_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_primary_selection_offer_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"receive".as_ptr(), signature: c"sh".as_ptr(), types: zwp_primary_selection_offer_v1_requests_receive_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_primary_selection_offer_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_primary_selection_offer_v1_events_offer_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_primary_selection_offer_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"offer".as_ptr(), signature: c"s".as_ptr(), types: zwp_primary_selection_offer_v1_events_offer_types.0.as_ptr() },
        ]);
        pub static zwp_primary_selection_offer_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_primary_selection_offer_v1".as_ptr(), version: 1, request_count: 2, requests: zwp_primary_selection_offer_v1_requests.0.as_ptr(), event_count: 1, events: zwp_primary_selection_offer_v1_events.0.as_ptr(),
        };
        pub static ZWP_PRIMARY_SELECTION_SOURCE_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zwp_primary_selection_source_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "offer", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "send", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Fd,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "cancelled", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zwp_primary_selection_source_v1_interface),
        };
        static zwp_primary_selection_source_v1_requests_offer_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zwp_primary_selection_source_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_primary_selection_source_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"offer".as_ptr(), signature: c"s".as_ptr(), types: zwp_primary_selection_source_v1_requests_offer_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zwp_primary_selection_source_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static zwp_primary_selection_source_v1_events_send_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
        static zwp_primary_selection_source_v1_events_cancelled_types: InterfacePointers = InterfacePointers(&[]);
        static zwp_primary_selection_source_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"send".as_ptr(), signature: c"sh".as_ptr(), types: zwp_primary_selection_source_v1_events_send_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"cancelled".as_ptr(), signature: c"".as_ptr(), types: zwp_primary_selection_source_v1_events_cancelled_types.0.as_ptr() },
        ]);
        pub static zwp_primary_selection_source_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zwp_primary_selection_source_v1".as_ptr(), version: 1, request_count: 2, requests: zwp_primary_selection_source_v1_requests.0.as_ptr(), event_count: 2, events: zwp_primary_selection_source_v1_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "X primary selection emulation\n\nThe primary selection device manager is a singleton global object that\nprovides access to the primary selection. It allows to create\nwp_primary_selection_source objects, as well as retrieving the per-seat\nwp_primary_selection_device objects."]
        pub mod zwp_primary_selection_device_manager_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "create a new primary selection source\n\nCreate a new primary selection source."]
            CreateSource {
            },
        #[doc = "create a new primary selection device\n\nCreate a new data device for a given seat."]
            GetDevice {
                seat: super::wl_seat::WlSeat,
            },
        #[doc = "destroy the primary selection device manager\n\n"]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::CreateSource { .. } => 0,
            Self::GetDevice { .. } => 1,
            Self::Destroy => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_CREATE_SOURCE_SINCE: u32 = 1;
        pub const REQ_CREATE_SOURCE_OPCODE: u16 = 0;
        pub const REQ_GET_DEVICE_SINCE: u32 = 1;
        pub const REQ_GET_DEVICE_OPCODE: u16 = 1;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct ZwpPrimarySelectionDeviceManagerV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPrimarySelectionDeviceManagerV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPrimarySelectionDeviceManagerV1 {}
        impl PartialEq<Weak<ZwpPrimarySelectionDeviceManagerV1>> for ZwpPrimarySelectionDeviceManagerV1 {
            fn eq(&self, other: &Weak<ZwpPrimarySelectionDeviceManagerV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPrimarySelectionDeviceManagerV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPrimarySelectionDeviceManagerV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPrimarySelectionDeviceManagerV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::CreateSource {  } => {
                    let child_spec = Some((super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    Ok((msg, child_spec))
                },
                Request::GetDevice { seat } => {
                    let child_spec = Some((super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(seat.id()));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPrimarySelectionDeviceManagerV1 {
        #[doc = "create a new primary selection source\n\nCreate a new primary selection source."]
        pub fn create_source<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, U> + 'static>(&self, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1 {
            self.send_constructor::<super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>(Request::CreateSource {  }, qh.make_data::<super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, U>(udata)).unwrap_or_else(|_| super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1::inert(self.backend.clone()))
        }
        #[doc = "create a new primary selection device\n\nCreate a new data device for a given seat."]
        pub fn get_device<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, U> + 'static>(&self, seat: &super::wl_seat::WlSeat, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 {
            self.send_constructor::<super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>(Request::GetDevice { seat: seat.clone() }, qh.make_data::<super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, U>(udata)).unwrap_or_else(|_| super::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1::inert(self.backend.clone()))
        }
        #[doc = "destroy the primary selection device manager\n\n"]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "offer primary selection data\n\nProvides access to the primary selection on the seat."]
        pub mod zwp_primary_selection_device_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "set the primary selection\n\nReplaces the current selection. The previous owner of the primary\nselection will receive a wp_primary_selection_source.cancelled event.\n\nTo unset the selection, set the source to NULL."]
            SetSelection {
                source: Option<super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
        #[doc = "serial of the event that triggered this request"]
                serial: u32,
            },
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetSelection { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_SELECTION_SINCE: u32 = 1;
        pub const REQ_SET_SELECTION_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "introduce a new wp_primary_selection_offer\n\nIntroduces a new wp_primary_selection_offer object that may be used\nto receive the current primary selection. Immediately following this\nevent, the new wp_primary_selection_offer object will send\nwp_primary_selection_offer.offer events to describe the offered mime\ntypes."]
            DataOffer {
                offer: super::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
            },
        #[doc = "advertise a new primary selection\n\nThe wp_primary_selection_device.selection event is sent to notify the\nclient of a new primary selection. This event is sent after the\nwp_primary_selection.data_offer event introducing this object, and after\nthe offer has announced its mime types through\nwp_primary_selection_offer.offer.\n\nThe data_offer is valid until a new offer or NULL is received\nor until the client loses keyboard focus. The client must destroy the\nprevious selection data_offer, if any, upon receiving this event."]
            Selection {
                id: Option<super::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::DataOffer { .. } => 0,
            Self::Selection { .. } => 1,
        } } }
        pub const EVT_DATA_OFFER_SINCE: u32 = 1;
        pub const EVT_DATA_OFFER_OPCODE: u16 = 0;
        pub const EVT_SELECTION_SINCE: u32 = 1;
        pub const EVT_SELECTION_OPCODE: u16 = 1;

        #[derive(Debug, Clone)]
        pub struct ZwpPrimarySelectionDeviceV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPrimarySelectionDeviceV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPrimarySelectionDeviceV1 {}
        impl PartialEq<Weak<ZwpPrimarySelectionDeviceV1>> for ZwpPrimarySelectionDeviceV1 {
            fn eq(&self, other: &Weak<ZwpPrimarySelectionDeviceV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPrimarySelectionDeviceV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPrimarySelectionDeviceV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPrimarySelectionDeviceV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_PRIMARY_SELECTION_DEVICE_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::NewId(offer)),) = (arg_iter.next(),) {
                        Ok((me, Event::DataOffer { offer: super::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1::from_id(conn, offer.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })? }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => {
                    if let (Some(Argument::Object(id)),) = (arg_iter.next(),) {
                        Ok((me, Event::Selection { id: if id.is_null() { None } else { Some(super::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1::from_id(conn, id.clone()).map_err(|_| DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode })?) } }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetSelection { source, serial } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Object(source.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                    msg.args.push(Argument::Uint(serial));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPrimarySelectionDeviceV1 {
        #[doc = "set the primary selection\n\nReplaces the current selection. The previous owner of the primary\nselection will receive a wp_primary_selection_source.cancelled event.\n\nTo unset the selection, set the source to NULL."]
        pub fn set_selection(&self, source: Option<&super::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>, serial: u32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetSelection { source: source.cloned(), serial: serial }, None);
        }
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "offer to transfer primary selection contents\n\nA wp_primary_selection_offer represents an offer to transfer the contents\nof the primary selection clipboard to the client.  Similar to\nwl_data_offer, the offer also describes the mime types that the data can\nbe converted to and provides the mechanisms for transferring the data\ndirectly to the client."]
        pub mod zwp_primary_selection_offer_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "request that the data is transferred\n\nTo transfer the contents of the primary selection clipboard, the client\nissues this request and indicates the mime type that it wants to\nreceive. The transfer happens through the passed file descriptor\n(typically created with the pipe system call). The source client writes\nthe data in the mime type representation requested and then closes the\nfile descriptor.\n\nThe receiving client reads from the read end of the pipe until EOF and\ncloses its end, at which point the transfer is complete."]
            Receive {
                mime_type: String,
                fd: std::os::fd::BorrowedFd<'a>,
            },
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Receive { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_RECEIVE_SINCE: u32 = 1;
        pub const REQ_RECEIVE_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "advertise offered mime type\n\nSent immediately after creating announcing the\nwp_primary_selection_offer through\nwp_primary_selection_device.data_offer. One event is sent per offered\nmime type."]
            Offer {
                mime_type: String,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Offer { .. } => 0,
        } } }
        pub const EVT_OFFER_SINCE: u32 = 1;
        pub const EVT_OFFER_OPCODE: u16 = 0;

        #[derive(Debug, Clone)]
        pub struct ZwpPrimarySelectionOfferV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPrimarySelectionOfferV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPrimarySelectionOfferV1 {}
        impl PartialEq<Weak<ZwpPrimarySelectionOfferV1>> for ZwpPrimarySelectionOfferV1 {
            fn eq(&self, other: &Weak<ZwpPrimarySelectionOfferV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPrimarySelectionOfferV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPrimarySelectionOfferV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPrimarySelectionOfferV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_PRIMARY_SELECTION_OFFER_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Str(mime_type)),) = (arg_iter.next(),) {
                        Ok((me, Event::Offer { mime_type: String::from_utf8_lossy(mime_type.as_ref().unwrap().as_bytes()).into_owned() }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Receive { mime_type, fd } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(mime_type).unwrap()))));
                    msg.args.push(Argument::Fd(fd));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPrimarySelectionOfferV1 {
        #[doc = "request that the data is transferred\n\nTo transfer the contents of the primary selection clipboard, the client\nissues this request and indicates the mime type that it wants to\nreceive. The transfer happens through the passed file descriptor\n(typically created with the pipe system call). The source client writes\nthe data in the mime type representation requested and then closes the\nfile descriptor.\n\nThe receiving client reads from the read end of the pipe until EOF and\ncloses its end, at which point the transfer is complete."]
        pub fn receive(&self, mime_type: String, fd: std::os::fd::BorrowedFd<'_>) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Receive { mime_type: mime_type, fd: fd }, None);
        }
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }
        #[doc = "offer to replace the contents of the primary selection\n\nThe source side of a wp_primary_selection_offer, it provides a way to\ndescribe the offered data and respond to requests to transfer the\nrequested contents of the primary selection clipboard."]
        pub mod zwp_primary_selection_source_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "add an offered mime type\n\nThis request adds a mime type to the set of mime types advertised to\ntargets. Can be called several times to offer multiple types."]
            Offer {
                mime_type: String,
            },
        #[doc = "destroy the primary selection source\n\nDestroy the primary selection source."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Offer { .. } => 0,
            Self::Destroy => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_OFFER_SINCE: u32 = 1;
        pub const REQ_OFFER_OPCODE: u16 = 0;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "send the primary selection contents\n\nRequest for the current primary selection contents from the client.\nSend the specified mime type over the passed file descriptor, then\nclose it."]
            Send {
                mime_type: String,
                fd: OwnedFd,
            },
        #[doc = "request for primary selection contents was canceled\n\nThis primary selection source is no longer valid. The client should\nclean up and destroy this primary selection source."]
            Cancelled,
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Send { .. } => 0,
            Self::Cancelled => 1,
        } } }
        pub const EVT_SEND_SINCE: u32 = 1;
        pub const EVT_SEND_OPCODE: u16 = 0;
        pub const EVT_CANCELLED_SINCE: u32 = 1;
        pub const EVT_CANCELLED_OPCODE: u16 = 1;

        #[derive(Debug, Clone)]
        pub struct ZwpPrimarySelectionSourceV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZwpPrimarySelectionSourceV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZwpPrimarySelectionSourceV1 {}
        impl PartialEq<Weak<ZwpPrimarySelectionSourceV1>> for ZwpPrimarySelectionSourceV1 {
            fn eq(&self, other: &Weak<ZwpPrimarySelectionSourceV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZwpPrimarySelectionSourceV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZwpPrimarySelectionSourceV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZwpPrimarySelectionSourceV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZWP_PRIMARY_SELECTION_SOURCE_V1_INTERFACE }
            #[inline]
            fn id(&self) -> ObjectId { self.id.clone() }
            #[inline]
            fn version(&self) -> u32 { self.version }
            #[inline]
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
            fn backend(&self) -> &WeakBackend { &self.backend }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, None)?;
                // A request without new_id never yields an object (checked in debug builds).
                #[cfg(debug_assertions)]
                if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
                let _ = id;
                Ok(())
            }
            fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            #[inline]
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
                let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
                let data = conn.get_object_data(id.clone()).ok();
                let backend = conn.backend().downgrade();
                Ok(Self { id, version, data, backend })
            }
            #[inline]
            fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
        fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
            let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
            let mut arg_iter = msg.args.into_iter();
            match msg.opcode {
                0 => {
                    if let (Some(Argument::Str(mime_type)), Some(Argument::Fd(fd)),) = (arg_iter.next(), arg_iter.next(),) {
                        Ok((me, Event::Send { mime_type: String::from_utf8_lossy(mime_type.as_ref().unwrap().as_bytes()).into_owned(), fd: fd }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => Ok((me, Event::Cancelled)),
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Offer { mime_type } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(mime_type).unwrap()))));
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZwpPrimarySelectionSourceV1 {
        #[doc = "add an offered mime type\n\nThis request adds a mime type to the set of mime types advertised to\ntargets. Can be called several times to offer multiple types."]
        pub fn offer(&self, mime_type: String) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Offer { mime_type: mime_type }, None);
        }
        #[doc = "destroy the primary selection source\n\nDestroy the primary selection source."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        }
        }

        }
    }
}

pub mod viewporter {
    pub mod client {
        #![allow(non_upper_case_globals)]
    // Copyright © 2013-2016 Collabora, Ltd.
    //
    // Permission is hereby granted, free of charge, to any person obtaining a
    // copy of this software and associated documentation files (the "Software"),
    // to deal in the Software without restriction, including without limitation
    // the rights to use, copy, modify, merge, publish, distribute, sublicense,
    // and/or sell copies of the Software, and to permit persons to whom the
    // Software is furnished to do so, subject to the following conditions:
    //
    // The above copyright notice and this permission notice (including the next
    // paragraph) shall be included in all copies or substantial portions of the
    // Software.
    //
    // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    // THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    // FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    // DEALINGS IN THE SOFTWARE.
        //! Client-side API of this protocol
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            // Immutable libwayland metadata, shared by every connection.
    struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
    unsafe impl Sync for InterfacePointers {}
    struct Messages(&'static [wayland_backend::protocol::wl_message]);
    unsafe impl Sync for Messages {}
    pub static WP_VIEWPORTER_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "wp_viewporter", version: 1,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "get_viewport", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                child_interface: Some(&WP_VIEWPORT_INTERFACE), arg_interfaces: &[&WL_SURFACE_INTERFACE,],
            },
        ],
        events: &[
        ],
        c_ptr: Some(&wp_viewporter_interface),
    };
    static wp_viewporter_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static wp_viewporter_requests_get_viewport_types: InterfacePointers = InterfacePointers(&[&wp_viewport_interface,&wl_surface_interface,]);
    static wp_viewporter_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_viewporter_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"get_viewport".as_ptr(), signature: c"no".as_ptr(), types: wp_viewporter_requests_get_viewport_types.0.as_ptr() },
    ]);
    pub static wp_viewporter_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"wp_viewporter".as_ptr(), version: 1, request_count: 2, requests: wp_viewporter_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
    };
    pub static WP_VIEWPORT_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "wp_viewport", version: 1,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_source", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,wayland_backend::protocol::ArgumentType::Fixed,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_destination", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        events: &[
        ],
        c_ptr: Some(&wp_viewport_interface),
    };
    static wp_viewport_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static wp_viewport_requests_set_source_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static wp_viewport_requests_set_destination_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static wp_viewport_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: wp_viewport_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_source".as_ptr(), signature: c"ffff".as_ptr(), types: wp_viewport_requests_set_source_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_destination".as_ptr(), signature: c"ii".as_ptr(), types: wp_viewport_requests_set_destination_types.0.as_ptr() },
    ]);
    pub static wp_viewport_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"wp_viewport".as_ptr(), version: 1, request_count: 3, requests: wp_viewport_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
    };

        }
        use self::__interfaces::*;
        #[doc = "surface cropping and scaling\n\nThe global interface exposing surface cropping and scaling\ncapabilities is used to instantiate an interface extension for a\nwl_surface object. This extended interface will then allow\ncropping and scaling the surface contents, effectively\ndisconnecting the direct relationship between the buffer and the\nsurface size."]
    pub mod wp_viewporter {
    use super::wayland_client;
    use std::sync::Arc;
    use std::os::fd::OwnedFd;
    use wayland_client::{Proxy, Connection, DispatchError, Weak};
    use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
    use wayland_client::backend::protocol::Argument;
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Error {
    #[doc = "the surface already has a viewport object associated"]
        ViewportExists = 0,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::ViewportExists),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "unbind from the cropping and scaling interface\n\nInforms the server that the client will not be using this\nprotocol object anymore. This does not affect any other objects,\nwp_viewport objects included."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "extend surface interface for crop and scale\n\nInstantiate an interface extension for the given wl_surface to\ncrop and scale its content. If the given wl_surface already has\na wp_viewport object associated, the viewport_exists\nprotocol error is raised."]
        GetViewport {
    #[doc = "the surface"]
            surface: super::wl_surface::WlSurface,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::GetViewport { .. } => 1,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_GET_VIEWPORT_SINCE: u32 = 1;
    pub const REQ_GET_VIEWPORT_OPCODE: u16 = 1;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
    } } }

    #[derive(Debug, Clone)]
    pub struct WpViewporter {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for WpViewporter { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for WpViewporter {}
    impl PartialEq<Weak<WpViewporter>> for WpViewporter {
        fn eq(&self, other: &Weak<WpViewporter>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for WpViewporter { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for WpViewporter {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for WpViewporter {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::WP_VIEWPORTER_INTERFACE }
        #[inline]
        fn id(&self) -> ObjectId { self.id.clone() }
        #[inline]
        fn version(&self) -> u32 { self.version }
        #[inline]
        fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
            self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
        }
        fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
        fn backend(&self) -> &WeakBackend { &self.backend }
        fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
            let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
            let id = conn.send_request(self, request, None)?;
            // A request without new_id never yields an object (checked in debug builds).
            #[cfg(debug_assertions)]
            if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
            let _ = id;
            Ok(())
        }
        fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
            let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
            let id = conn.send_request(self, request, Some(data))?;
            I::from_id(&conn, id)
        }
        #[inline]
        fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
            if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
            let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
            let data = conn.get_object_data(id.clone()).ok();
            let backend = conn.backend().downgrade();
            Ok(Self { id, version, data, backend })
        }
        #[inline]
        fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
    fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
        let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
    }
    fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
        match request {
            Request::Destroy => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::GetViewport { surface } => {
                let child_spec = Some((super::wp_viewport::WpViewport::interface(), conn.object_info(self.id())?.version));
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::NewId(ObjectId::null()));
                msg.args.push(Argument::Object(surface.id()));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl WpViewporter {
    #[doc = "unbind from the cropping and scaling interface\n\nInforms the server that the client will not be using this\nprotocol object anymore. This does not affect any other objects,\nwp_viewport objects included."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "extend surface interface for crop and scale\n\nInstantiate an interface extension for the given wl_surface to\ncrop and scale its content. If the given wl_surface already has\na wp_viewport object associated, the viewport_exists\nprotocol error is raised."]
    pub fn get_viewport<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::wp_viewport::WpViewport, U> + 'static>(&self, surface: &super::wl_surface::WlSurface, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::wp_viewport::WpViewport {
        self.send_constructor::<super::wp_viewport::WpViewport>(Request::GetViewport { surface: surface.clone() }, qh.make_data::<super::wp_viewport::WpViewport, U>(udata)).unwrap_or_else(|_| super::wp_viewport::WpViewport::inert(self.backend.clone()))
    }
    }
    }
    #[doc = "crop and scale interface to a wl_surface\n\nAn additional interface to a wl_surface object, which allows the\nclient to specify the cropping and scaling of the surface\ncontents.\n\nThis interface works with two concepts: the source rectangle (src_x,\nsrc_y, src_width, src_height), and the destination size (dst_width,\ndst_height). The contents of the source rectangle are scaled to the\ndestination size, and content outside the source rectangle is ignored.\nThis state is double-buffered, see wl_surface.commit.\n\nThe two parts of crop and scale state are independent: the source\nrectangle, and the destination size. Initially both are unset, that\nis, no scaling is applied. The whole of the current wl_buffer is\nused as the source, and the surface size is as defined in\nwl_surface.attach.\n\nIf the destination size is set, it causes the surface size to become\ndst_width, dst_height. The source (rectangle) is scaled to exactly\nthis size. This overrides whatever the attached wl_buffer size is,\nunless the wl_buffer is NULL. If the wl_buffer is NULL, the surface\nhas no content and therefore no size. Otherwise, the size is always\nat least 1x1 in surface local coordinates.\n\nIf the source rectangle is set, it defines what area of the wl_buffer is\ntaken as the source. If the source rectangle is set and the destination\nsize is not set, then src_width and src_height must be integers, and the\nsurface size becomes the source rectangle size. This results in cropping\nwithout scaling. If src_width or src_height are not integers and\ndestination size is not set, the bad_size protocol error is raised when\nthe surface state is applied.\n\nThe coordinate transformations from buffer pixel coordinates up to\nthe surface-local coordinates happen in the following order:\n1. buffer_transform (wl_surface.set_buffer_transform)\n2. buffer_scale (wl_surface.set_buffer_scale)\n3. crop and scale (wp_viewport.set*)\nThis means, that the source rectangle coordinates of crop and scale\nare given in the coordinates after the buffer transform and scale,\ni.e. in the coordinates that would be the surface-local coordinates\nif the crop and scale was not applied.\n\nIf src_x or src_y are negative, the bad_value protocol error is raised.\nOtherwise, if the source rectangle is partially or completely outside of\nthe non-NULL wl_buffer, then the out_of_buffer protocol error is raised\nwhen the surface state is applied. A NULL wl_buffer does not raise the\nout_of_buffer error.\n\nIf the wl_surface associated with the wp_viewport is destroyed,\nall wp_viewport requests except 'destroy' raise the protocol error\nno_surface.\n\nIf the wp_viewport object is destroyed, the crop and scale\nstate is removed from the wl_surface. The change will be applied\non the next wl_surface.commit."]
    pub mod wp_viewport {
    use super::wayland_client;
    use std::sync::Arc;
    use std::os::fd::OwnedFd;
    use wayland_client::{Proxy, Connection, DispatchError, Weak};
    use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
    use wayland_client::backend::protocol::Argument;
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Error {
    #[doc = "negative or zero values in width or height"]
        BadValue = 0,
    #[doc = "destination size is not integer"]
        BadSize = 1,
    #[doc = "source rectangle extends outside of the content area"]
        OutOfBuffer = 2,
    #[doc = "the wl_surface was destroyed"]
        NoSurface = 3,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::BadValue),
            1 => Ok(Self::BadSize),
            2 => Ok(Self::OutOfBuffer),
            3 => Ok(Self::NoSurface),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "remove scaling and cropping from the surface\n\nThe associated wl_surface's crop and scale state is removed.\nThe change is applied on the next wl_surface.commit."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "set the source rectangle for cropping\n\nSet the source rectangle of the associated wl_surface. See\nwp_viewport for the description, and relation to the wl_buffer\nsize.\n\nIf all of x, y, width and height are -1.0, the source rectangle is\nunset instead. Any other set of values where width or height are zero\nor negative, or x or y are negative, raise the bad_value protocol\nerror.\n\nThe crop and scale state is double-buffered, see wl_surface.commit."]
        SetSource {
    #[doc = "source rectangle x"]
            x: f64,
    #[doc = "source rectangle y"]
            y: f64,
    #[doc = "source rectangle width"]
            width: f64,
    #[doc = "source rectangle height"]
            height: f64,
        },
    #[doc = "set the surface size for scaling\n\nSet the destination size of the associated wl_surface. See\nwp_viewport for the description, and relation to the wl_buffer\nsize.\n\nIf width is -1 and height is -1, the destination size is unset\ninstead. Any other pair of values for width and height that\ncontains zero or negative values raises the bad_value protocol\nerror.\n\nThe crop and scale state is double-buffered, see wl_surface.commit."]
        SetDestination {
    #[doc = "surface width"]
            width: i32,
    #[doc = "surface height"]
            height: i32,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::SetSource { .. } => 1,
        Self::SetDestination { .. } => 2,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_SET_SOURCE_SINCE: u32 = 1;
    pub const REQ_SET_SOURCE_OPCODE: u16 = 1;
    pub const REQ_SET_DESTINATION_SINCE: u32 = 1;
    pub const REQ_SET_DESTINATION_OPCODE: u16 = 2;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
    } } }

    #[derive(Debug, Clone)]
    pub struct WpViewport {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for WpViewport { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for WpViewport {}
    impl PartialEq<Weak<WpViewport>> for WpViewport {
        fn eq(&self, other: &Weak<WpViewport>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for WpViewport { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for WpViewport {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for WpViewport {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::WP_VIEWPORT_INTERFACE }
        #[inline]
        fn id(&self) -> ObjectId { self.id.clone() }
        #[inline]
        fn version(&self) -> u32 { self.version }
        #[inline]
        fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
            self.data.as_ref().and_then(|arc| arc.data_as_any().downcast_ref::<U>())
        }
        fn object_data(&self) -> Option<&Arc<dyn ObjectData>> { self.data.as_ref() }
        fn backend(&self) -> &WeakBackend { &self.backend }
        fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
            let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
            let id = conn.send_request(self, request, None)?;
            // A request without new_id never yields an object (checked in debug builds).
            #[cfg(debug_assertions)]
            if !id.is_null() { std::panic::panic_any("send_request returned an object id for a request without new_id"); }
            let _ = id;
            Ok(())
        }
        fn send_constructor<I: Proxy>(&self, request: Self::Request<'_>, data: Arc<dyn ObjectData>) -> Result<I, InvalidId> {
            let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
            let id = conn.send_request(self, request, Some(data))?;
            I::from_id(&conn, id)
        }
        #[inline]
        fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
            if !same_interface(id.interface(), Self::interface()) && !id.is_null() { return Err(InvalidId); }
            let version = conn.object_info(id.clone()).map(|info| info.version).unwrap_or(0);
            let data = conn.get_object_data(id.clone()).ok();
            let backend = conn.backend().downgrade();
            Ok(Self { id, version, data, backend })
        }
        #[inline]
        fn inert(backend: WeakBackend) -> Self { Self { id: ObjectId::null(), version: 0, data: None, backend } }
    fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {
        let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();
        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
    }
    fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
        match request {
            Request::Destroy => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::SetSource { x, y, width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::Fixed((x * 256.0) as i32));
                msg.args.push(Argument::Fixed((y * 256.0) as i32));
                msg.args.push(Argument::Fixed((width * 256.0) as i32));
                msg.args.push(Argument::Fixed((height * 256.0) as i32));
                Ok((msg, child_spec))
            },
            Request::SetDestination { width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl WpViewport {
    #[doc = "remove scaling and cropping from the surface\n\nThe associated wl_surface's crop and scale state is removed.\nThe change is applied on the next wl_surface.commit."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "set the source rectangle for cropping\n\nSet the source rectangle of the associated wl_surface. See\nwp_viewport for the description, and relation to the wl_buffer\nsize.\n\nIf all of x, y, width and height are -1.0, the source rectangle is\nunset instead. Any other set of values where width or height are zero\nor negative, or x or y are negative, raise the bad_value protocol\nerror.\n\nThe crop and scale state is double-buffered, see wl_surface.commit."]
    pub fn set_source(&self, x: f64, y: f64, width: f64, height: f64) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetSource { x: x, y: y, width: width, height: height }, None);
    }
    #[doc = "set the surface size for scaling\n\nSet the destination size of the associated wl_surface. See\nwp_viewport for the description, and relation to the wl_buffer\nsize.\n\nIf width is -1 and height is -1, the destination size is unset\ninstead. Any other pair of values for width and height that\ncontains zero or negative values raises the bad_value protocol\nerror.\n\nThe crop and scale state is double-buffered, see wl_surface.commit."]
    pub fn set_destination(&self, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetDestination { width: width, height: height }, None);
    }
    }
    }

    }
}
