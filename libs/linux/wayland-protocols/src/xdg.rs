//! Generated from protocol XML by `tools/wayland_codegen`.
//! Do not edit manually; regenerate instead.

#[cfg(feature = "unstable")]
pub mod decoration {
    pub mod zv1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2018 Simon Ser
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
            use crate::xdg::shell::{client::*};
            pub mod __interfaces {
                use crate::xdg::shell::{client::__interfaces::*};
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static ZXDG_DECORATION_MANAGER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zxdg_decoration_manager_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_toplevel_decoration", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: Some(&ZXDG_TOPLEVEL_DECORATION_V1_INTERFACE), arg_interfaces: &[&XDG_TOPLEVEL_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&zxdg_decoration_manager_v1_interface),
        };
        static zxdg_decoration_manager_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zxdg_decoration_manager_v1_requests_get_toplevel_decoration_types: InterfacePointers = InterfacePointers(&[&zxdg_toplevel_decoration_v1_interface,&xdg_toplevel_interface,]);
        static zxdg_decoration_manager_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zxdg_decoration_manager_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_toplevel_decoration".as_ptr(), signature: c"no".as_ptr(), types: zxdg_decoration_manager_v1_requests_get_toplevel_decoration_types.0.as_ptr() },
        ]);
        pub static zxdg_decoration_manager_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zxdg_decoration_manager_v1".as_ptr(), version: 1, request_count: 2, requests: zxdg_decoration_manager_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static ZXDG_TOPLEVEL_DECORATION_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "zxdg_toplevel_decoration_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_mode", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "unset_mode", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "configure", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&zxdg_toplevel_decoration_v1_interface),
        };
        static zxdg_toplevel_decoration_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static zxdg_toplevel_decoration_v1_requests_set_mode_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zxdg_toplevel_decoration_v1_requests_unset_mode_types: InterfacePointers = InterfacePointers(&[]);
        static zxdg_toplevel_decoration_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: zxdg_toplevel_decoration_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_mode".as_ptr(), signature: c"u".as_ptr(), types: zxdg_toplevel_decoration_v1_requests_set_mode_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"unset_mode".as_ptr(), signature: c"".as_ptr(), types: zxdg_toplevel_decoration_v1_requests_unset_mode_types.0.as_ptr() },
        ]);
        static zxdg_toplevel_decoration_v1_events_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static zxdg_toplevel_decoration_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"configure".as_ptr(), signature: c"u".as_ptr(), types: zxdg_toplevel_decoration_v1_events_configure_types.0.as_ptr() },
        ]);
        pub static zxdg_toplevel_decoration_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"zxdg_toplevel_decoration_v1".as_ptr(), version: 1, request_count: 3, requests: zxdg_toplevel_decoration_v1_requests.0.as_ptr(), event_count: 1, events: zxdg_toplevel_decoration_v1_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "window decoration manager\n\nThis interface allows a compositor to announce support for server-side\ndecorations.\n\nA window decoration is a set of window controls as deemed appropriate by\nthe party managing them, such as user interface components used to move,\nresize and change a window's state.\n\nA client can use this protocol to request being decorated by a supporting\ncompositor.\n\nIf compositor and client do not negotiate the use of a server-side\ndecoration using this protocol, clients continue to self-decorate as they\nsee fit.\n\nWarning! The protocol described in this file is experimental and\nbackward incompatible changes may be made. Backward compatible changes\nmay be added together with the corresponding interface version bump.\nBackward incompatible changes are done by bumping the version number in\nthe protocol and interface names and resetting the interface version.\nOnce the protocol is to be declared stable, the 'z' prefix and the\nversion number in the protocol and interface names are removed and the\ninterface version number is reset."]
        pub mod zxdg_decoration_manager_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the decoration manager object\n\nDestroy the decoration manager. This doesn't destroy objects created\nwith the manager."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "create a new toplevel decoration object\n\nCreate a new decoration object associated with the given toplevel.\n\nCreating an xdg_toplevel_decoration from an xdg_toplevel which has a\nbuffer attached or committed is a client error, and any attempts by a\nclient to attach or manipulate a buffer prior to the first\nxdg_toplevel_decoration.configure event must also be treated as\nerrors."]
            GetToplevelDecoration {
                toplevel: super::xdg_toplevel::XdgToplevel,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::GetToplevelDecoration { .. } => 1,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_GET_TOPLEVEL_DECORATION_SINCE: u32 = 1;
        pub const REQ_GET_TOPLEVEL_DECORATION_OPCODE: u16 = 1;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct ZxdgDecorationManagerV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZxdgDecorationManagerV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZxdgDecorationManagerV1 {}
        impl PartialEq<Weak<ZxdgDecorationManagerV1>> for ZxdgDecorationManagerV1 {
            fn eq(&self, other: &Weak<ZxdgDecorationManagerV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZxdgDecorationManagerV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZxdgDecorationManagerV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZxdgDecorationManagerV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZXDG_DECORATION_MANAGER_V1_INTERFACE }
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
                Request::GetToplevelDecoration { toplevel } => {
                    let child_spec = Some((super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    msg.args.push(Argument::Object(toplevel.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZxdgDecorationManagerV1 {
        #[doc = "destroy the decoration manager object\n\nDestroy the decoration manager. This doesn't destroy objects created\nwith the manager."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "create a new toplevel decoration object\n\nCreate a new decoration object associated with the given toplevel.\n\nCreating an xdg_toplevel_decoration from an xdg_toplevel which has a\nbuffer attached or committed is a client error, and any attempts by a\nclient to attach or manipulate a buffer prior to the first\nxdg_toplevel_decoration.configure event must also be treated as\nerrors."]
        pub fn get_toplevel_decoration<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, U> + 'static>(&self, toplevel: &super::xdg_toplevel::XdgToplevel, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1 {
            self.send_constructor::<super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>(Request::GetToplevelDecoration { toplevel: toplevel.clone() }, qh.make_data::<super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, U>(udata)).unwrap_or_else(|_| super::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1::inert(self.backend.clone()))
        }
        }
        }
        #[doc = "decoration object for a toplevel surface\n\nThe decoration object allows the compositor to toggle server-side window\ndecorations for a toplevel surface. The client can request to switch to\nanother mode.\n\nThe xdg_toplevel_decoration object must be destroyed before its\nxdg_toplevel."]
        pub mod zxdg_toplevel_decoration_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        use wayland_client::backend::protocol::WEnum;
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Error {
        #[doc = "xdg_toplevel has a buffer attached before configure"]
            UnconfiguredBuffer = 0,
        #[doc = "xdg_toplevel already has a decoration object"]
            AlreadyConstructed = 1,
        #[doc = "xdg_toplevel destroyed before the decoration object"]
            Orphaned = 2,
        #[doc = "invalid mode"]
            InvalidMode = 3,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::UnconfiguredBuffer),
                1 => Ok(Self::AlreadyConstructed),
                2 => Ok(Self::Orphaned),
                3 => Ok(Self::InvalidMode),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[doc = "window decoration modes\n\nThese values describe window decoration modes."]
        #[repr(u32)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum Mode {
        #[doc = "no server-side window decoration"]
            ClientSide = 1,
        #[doc = "server-side window decoration"]
            ServerSide = 2,
        }
        impl TryFrom<u32> for Mode {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::ClientSide),
                2 => Ok(Self::ServerSide),
                _ => Err(()),
            } }
        }
        impl From<Mode> for u32 { fn from(value: Mode) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the decoration object\n\nSwitch back to a mode without any server-side decorations at the next\ncommit."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "set the decoration mode\n\nSet the toplevel surface decoration mode. This informs the compositor\nthat the client prefers the provided decoration mode.\n\nAfter requesting a decoration mode, the compositor will respond by\nemitting an xdg_surface.configure event. The client should then update\nits content, drawing it without decorations if the received mode is\nserver-side decorations. The client must also acknowledge the configure\nwhen committing the new content (see xdg_surface.ack_configure).\n\nThe compositor can decide not to use the client's mode and enforce a\ndifferent mode instead.\n\nClients whose decoration mode depend on the xdg_toplevel state may send\na set_mode request in response to an xdg_surface.configure event and wait\nfor the next xdg_surface.configure event to prevent unwanted state.\nSuch clients are responsible for preventing configure loops and must\nmake sure not to send multiple successive set_mode requests with the\nsame decoration mode.\n\nIf an invalid mode is supplied by the client, the invalid_mode protocol\nerror is raised by the compositor."]
            SetMode {
        #[doc = "the decoration mode"]
                mode: WEnum<Mode>,
            },
        #[doc = "unset the decoration mode\n\nUnset the toplevel surface decoration mode. This informs the compositor\nthat the client doesn't prefer a particular decoration mode.\n\nThis request has the same semantics as set_mode."]
            UnsetMode,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::SetMode { .. } => 1,
            Self::UnsetMode => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_SET_MODE_SINCE: u32 = 1;
        pub const REQ_SET_MODE_OPCODE: u16 = 1;
        pub const REQ_UNSET_MODE_SINCE: u32 = 1;
        pub const REQ_UNSET_MODE_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "notify a decoration mode change\n\nThe configure event configures the effective decoration mode. The\nconfigured state should not be applied immediately. Clients must send an\nack_configure in response to this event. See xdg_surface.configure and\nxdg_surface.ack_configure for details.\n\nA configure event can be sent at any time. The specified mode must be\nobeyed by the client."]
            Configure {
        #[doc = "the decoration mode"]
                mode: WEnum<Mode>,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Configure { .. } => 0,
        } } }
        pub const EVT_CONFIGURE_SINCE: u32 = 1;
        pub const EVT_CONFIGURE_OPCODE: u16 = 0;

        #[derive(Debug, Clone)]
        pub struct ZxdgToplevelDecorationV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for ZxdgToplevelDecorationV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for ZxdgToplevelDecorationV1 {}
        impl PartialEq<Weak<ZxdgToplevelDecorationV1>> for ZxdgToplevelDecorationV1 {
            fn eq(&self, other: &Weak<ZxdgToplevelDecorationV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for ZxdgToplevelDecorationV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for ZxdgToplevelDecorationV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for ZxdgToplevelDecorationV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::ZXDG_TOPLEVEL_DECORATION_V1_INTERFACE }
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
                    if let (Some(Argument::Uint(mode)),) = (arg_iter.next(),) {
                        Ok((me, Event::Configure { mode: WEnum::from(mode as u32) }))
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
                Request::SetMode { mode } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::Uint(u32::from(mode)));
                    Ok((msg, child_spec))
                },
                Request::UnsetMode => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl ZxdgToplevelDecorationV1 {
        #[doc = "destroy the decoration object\n\nSwitch back to a mode without any server-side decorations at the next\ncommit."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "set the decoration mode\n\nSet the toplevel surface decoration mode. This informs the compositor\nthat the client prefers the provided decoration mode.\n\nAfter requesting a decoration mode, the compositor will respond by\nemitting an xdg_surface.configure event. The client should then update\nits content, drawing it without decorations if the received mode is\nserver-side decorations. The client must also acknowledge the configure\nwhen committing the new content (see xdg_surface.ack_configure).\n\nThe compositor can decide not to use the client's mode and enforce a\ndifferent mode instead.\n\nClients whose decoration mode depend on the xdg_toplevel state may send\na set_mode request in response to an xdg_surface.configure event and wait\nfor the next xdg_surface.configure event to prevent unwanted state.\nSuch clients are responsible for preventing configure loops and must\nmake sure not to send multiple successive set_mode requests with the\nsame decoration mode.\n\nIf an invalid mode is supplied by the client, the invalid_mode protocol\nerror is raised by the compositor."]
        pub fn set_mode(&self, mode: Mode) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetMode { mode: WEnum::Value(mode) }, None);
        }
        #[doc = "unset the decoration mode\n\nUnset the toplevel surface decoration mode. This informs the compositor\nthat the client doesn't prefer a particular decoration mode.\n\nThis request has the same semantics as set_mode."]
        pub fn unset_mode(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::UnsetMode, None);
        }
        }
        }

        }
    }
}

pub mod shell {
    pub mod client {
        #![allow(non_upper_case_globals)]
    // Copyright © 2008-2013 Kristian Høgsberg
    // Copyright © 2013      Rafael Antognolli
    // Copyright © 2013      Jasper St. Pierre
    // Copyright © 2010-2013 Intel Corporation
    // Copyright © 2015-2017 Samsung Electronics Co., Ltd
    // Copyright © 2015-2017 Red Hat Inc.
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
    pub static XDG_WM_BASE_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "xdg_wm_base", version: 7,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "create_positioner", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                child_interface: Some(&XDG_POSITIONER_INTERFACE), arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "get_xdg_surface", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                child_interface: Some(&XDG_SURFACE_INTERFACE), arg_interfaces: &[&WL_SURFACE_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "pong", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        events: &[
            wayland_backend::protocol::MessageDesc { name: "ping", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        c_ptr: Some(&xdg_wm_base_interface),
    };
    static xdg_wm_base_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_wm_base_requests_create_positioner_types: InterfacePointers = InterfacePointers(&[&xdg_positioner_interface,]);
    static xdg_wm_base_requests_get_xdg_surface_types: InterfacePointers = InterfacePointers(&[&xdg_surface_interface,&wl_surface_interface,]);
    static xdg_wm_base_requests_pong_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_wm_base_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_wm_base_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"create_positioner".as_ptr(), signature: c"n".as_ptr(), types: xdg_wm_base_requests_create_positioner_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"get_xdg_surface".as_ptr(), signature: c"no".as_ptr(), types: xdg_wm_base_requests_get_xdg_surface_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"pong".as_ptr(), signature: c"u".as_ptr(), types: xdg_wm_base_requests_pong_types.0.as_ptr() },
    ]);
    static xdg_wm_base_events_ping_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_wm_base_events: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"ping".as_ptr(), signature: c"u".as_ptr(), types: xdg_wm_base_events_ping_types.0.as_ptr() },
    ]);
    pub static xdg_wm_base_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"xdg_wm_base".as_ptr(), version: 7, request_count: 4, requests: xdg_wm_base_requests.0.as_ptr(), event_count: 1, events: xdg_wm_base_events.0.as_ptr(),
    };
    pub static XDG_POSITIONER_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "xdg_positioner", version: 7,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_size", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_anchor_rect", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_anchor", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_gravity", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_constraint_adjustment", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_offset", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_reactive", since: 3, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_parent_size", since: 3, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_parent_configure", since: 3, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        events: &[
        ],
        c_ptr: Some(&xdg_positioner_interface),
    };
    static xdg_positioner_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_positioner_requests_set_size_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_positioner_requests_set_anchor_rect_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static xdg_positioner_requests_set_anchor_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_positioner_requests_set_gravity_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_positioner_requests_set_constraint_adjustment_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_positioner_requests_set_offset_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_positioner_requests_set_reactive_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_positioner_requests_set_parent_size_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_positioner_requests_set_parent_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_positioner_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_positioner_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_size".as_ptr(), signature: c"ii".as_ptr(), types: xdg_positioner_requests_set_size_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_anchor_rect".as_ptr(), signature: c"iiii".as_ptr(), types: xdg_positioner_requests_set_anchor_rect_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_anchor".as_ptr(), signature: c"u".as_ptr(), types: xdg_positioner_requests_set_anchor_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_gravity".as_ptr(), signature: c"u".as_ptr(), types: xdg_positioner_requests_set_gravity_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_constraint_adjustment".as_ptr(), signature: c"u".as_ptr(), types: xdg_positioner_requests_set_constraint_adjustment_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_offset".as_ptr(), signature: c"ii".as_ptr(), types: xdg_positioner_requests_set_offset_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_reactive".as_ptr(), signature: c"3".as_ptr(), types: xdg_positioner_requests_set_reactive_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_parent_size".as_ptr(), signature: c"3ii".as_ptr(), types: xdg_positioner_requests_set_parent_size_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_parent_configure".as_ptr(), signature: c"3u".as_ptr(), types: xdg_positioner_requests_set_parent_configure_types.0.as_ptr() },
    ]);
    pub static xdg_positioner_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"xdg_positioner".as_ptr(), version: 7, request_count: 10, requests: xdg_positioner_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
    };
    pub static XDG_SURFACE_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "xdg_surface", version: 7,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "get_toplevel", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                child_interface: Some(&XDG_TOPLEVEL_INTERFACE), arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "get_popup", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::NewId,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                child_interface: Some(&XDG_POPUP_INTERFACE), arg_interfaces: &[&XDG_SURFACE_INTERFACE,&XDG_POSITIONER_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "set_window_geometry", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "ack_configure", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        events: &[
            wayland_backend::protocol::MessageDesc { name: "configure", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        c_ptr: Some(&xdg_surface_interface),
    };
    static xdg_surface_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_surface_requests_get_toplevel_types: InterfacePointers = InterfacePointers(&[&xdg_toplevel_interface,]);
    static xdg_surface_requests_get_popup_types: InterfacePointers = InterfacePointers(&[&xdg_popup_interface,&xdg_surface_interface,&xdg_positioner_interface,]);
    static xdg_surface_requests_set_window_geometry_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static xdg_surface_requests_ack_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_surface_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_surface_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"get_toplevel".as_ptr(), signature: c"n".as_ptr(), types: xdg_surface_requests_get_toplevel_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"get_popup".as_ptr(), signature: c"n?oo".as_ptr(), types: xdg_surface_requests_get_popup_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_window_geometry".as_ptr(), signature: c"iiii".as_ptr(), types: xdg_surface_requests_set_window_geometry_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"ack_configure".as_ptr(), signature: c"u".as_ptr(), types: xdg_surface_requests_ack_configure_types.0.as_ptr() },
    ]);
    static xdg_surface_events_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_surface_events: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"configure".as_ptr(), signature: c"u".as_ptr(), types: xdg_surface_events_configure_types.0.as_ptr() },
    ]);
    pub static xdg_surface_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"xdg_surface".as_ptr(), version: 7, request_count: 5, requests: xdg_surface_requests.0.as_ptr(), event_count: 1, events: xdg_surface_events.0.as_ptr(),
    };
    pub static XDG_TOPLEVEL_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "xdg_toplevel", version: 7,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_parent", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),],
                child_interface: None, arg_interfaces: &[&XDG_TOPLEVEL_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "set_title", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_app_id", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "show_window_menu", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[&WL_SEAT_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "move", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[&WL_SEAT_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "resize", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[&WL_SEAT_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "set_max_size", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_min_size", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_maximized", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "unset_maximized", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_fullscreen", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),],
                child_interface: None, arg_interfaces: &[&WL_OUTPUT_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "unset_fullscreen", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "set_minimized", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        events: &[
            wayland_backend::protocol::MessageDesc { name: "configure", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Array,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "close", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "configure_bounds", since: 4, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "wm_capabilities", since: 5, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Array,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        c_ptr: Some(&xdg_toplevel_interface),
    };
    static xdg_toplevel_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_requests_set_parent_types: InterfacePointers = InterfacePointers(&[&xdg_toplevel_interface,]);
    static xdg_toplevel_requests_set_title_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_toplevel_requests_set_app_id_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_toplevel_requests_show_window_menu_types: InterfacePointers = InterfacePointers(&[&wl_seat_interface,core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_requests_move_types: InterfacePointers = InterfacePointers(&[&wl_seat_interface,core::ptr::null(),]);
    static xdg_toplevel_requests_resize_types: InterfacePointers = InterfacePointers(&[&wl_seat_interface,core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_requests_set_max_size_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_requests_set_min_size_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_requests_set_maximized_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_requests_unset_maximized_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_requests_set_fullscreen_types: InterfacePointers = InterfacePointers(&[&wl_output_interface,]);
    static xdg_toplevel_requests_unset_fullscreen_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_requests_set_minimized_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_parent".as_ptr(), signature: c"?o".as_ptr(), types: xdg_toplevel_requests_set_parent_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_title".as_ptr(), signature: c"s".as_ptr(), types: xdg_toplevel_requests_set_title_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_app_id".as_ptr(), signature: c"s".as_ptr(), types: xdg_toplevel_requests_set_app_id_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"show_window_menu".as_ptr(), signature: c"ouii".as_ptr(), types: xdg_toplevel_requests_show_window_menu_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"move".as_ptr(), signature: c"ou".as_ptr(), types: xdg_toplevel_requests_move_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"resize".as_ptr(), signature: c"ouu".as_ptr(), types: xdg_toplevel_requests_resize_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_max_size".as_ptr(), signature: c"ii".as_ptr(), types: xdg_toplevel_requests_set_max_size_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_min_size".as_ptr(), signature: c"ii".as_ptr(), types: xdg_toplevel_requests_set_min_size_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_maximized".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_requests_set_maximized_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"unset_maximized".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_requests_unset_maximized_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_fullscreen".as_ptr(), signature: c"?o".as_ptr(), types: xdg_toplevel_requests_set_fullscreen_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"unset_fullscreen".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_requests_unset_fullscreen_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"set_minimized".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_requests_set_minimized_types.0.as_ptr() },
    ]);
    static xdg_toplevel_events_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_events_close_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_toplevel_events_configure_bounds_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),]);
    static xdg_toplevel_events_wm_capabilities_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_toplevel_events: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"configure".as_ptr(), signature: c"iia".as_ptr(), types: xdg_toplevel_events_configure_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"close".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_events_close_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"configure_bounds".as_ptr(), signature: c"4ii".as_ptr(), types: xdg_toplevel_events_configure_bounds_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"wm_capabilities".as_ptr(), signature: c"5a".as_ptr(), types: xdg_toplevel_events_wm_capabilities_types.0.as_ptr() },
    ]);
    pub static xdg_toplevel_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"xdg_toplevel".as_ptr(), version: 7, request_count: 14, requests: xdg_toplevel_requests.0.as_ptr(), event_count: 4, events: xdg_toplevel_events.0.as_ptr(),
    };
    pub static XDG_POPUP_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
        name: "xdg_popup", version: 7,
        requests: &[
            wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "grab", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[&WL_SEAT_INTERFACE,],
            },
            wayland_backend::protocol::MessageDesc { name: "reposition", since: 3, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[&XDG_POSITIONER_INTERFACE,],
            },
        ],
        events: &[
            wayland_backend::protocol::MessageDesc { name: "configure", since: 1, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,wayland_backend::protocol::ArgumentType::Int,],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "popup_done", since: 1, is_destructor: false,
                signature: &[],
                child_interface: None, arg_interfaces: &[],
            },
            wayland_backend::protocol::MessageDesc { name: "repositioned", since: 3, is_destructor: false,
                signature: &[wayland_backend::protocol::ArgumentType::Uint,],
                child_interface: None, arg_interfaces: &[],
            },
        ],
        c_ptr: Some(&xdg_popup_interface),
    };
    static xdg_popup_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_popup_requests_grab_types: InterfacePointers = InterfacePointers(&[&wl_seat_interface,core::ptr::null(),]);
    static xdg_popup_requests_reposition_types: InterfacePointers = InterfacePointers(&[&xdg_positioner_interface,core::ptr::null(),]);
    static xdg_popup_requests: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_popup_requests_destroy_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"grab".as_ptr(), signature: c"ou".as_ptr(), types: xdg_popup_requests_grab_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"reposition".as_ptr(), signature: c"3ou".as_ptr(), types: xdg_popup_requests_reposition_types.0.as_ptr() },
    ]);
    static xdg_popup_events_configure_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),core::ptr::null(),core::ptr::null(),core::ptr::null(),]);
    static xdg_popup_events_popup_done_types: InterfacePointers = InterfacePointers(&[]);
    static xdg_popup_events_repositioned_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
    static xdg_popup_events: Messages = Messages(&[
        wayland_backend::protocol::wl_message { name: c"configure".as_ptr(), signature: c"iiii".as_ptr(), types: xdg_popup_events_configure_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"popup_done".as_ptr(), signature: c"".as_ptr(), types: xdg_popup_events_popup_done_types.0.as_ptr() },
        wayland_backend::protocol::wl_message { name: c"repositioned".as_ptr(), signature: c"3u".as_ptr(), types: xdg_popup_events_repositioned_types.0.as_ptr() },
    ]);
    pub static xdg_popup_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
        name: c"xdg_popup".as_ptr(), version: 7, request_count: 3, requests: xdg_popup_requests.0.as_ptr(), event_count: 3, events: xdg_popup_events.0.as_ptr(),
    };

        }
        use self::__interfaces::*;
        #[doc = "create desktop-style surfaces\n\nThe xdg_wm_base interface is exposed as a global object enabling clients\nto turn their wl_surfaces into windows in a desktop environment. It\ndefines the basic functionality needed for clients and the compositor to\ncreate windows that can be dragged, resized, maximized, etc, as well as\ncreating transient windows such as popup menus."]
    pub mod xdg_wm_base {
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
    #[doc = "given wl_surface has another role"]
        Role = 0,
    #[doc = "xdg_wm_base was destroyed before children"]
        DefunctSurfaces = 1,
    #[doc = "the client tried to map or destroy a non-topmost popup"]
        NotTheTopmostPopup = 2,
    #[doc = "the client specified an invalid popup parent surface"]
        InvalidPopupParent = 3,
    #[doc = "the client provided an invalid surface state"]
        InvalidSurfaceState = 4,
    #[doc = "the client provided an invalid positioner"]
        InvalidPositioner = 5,
    #[doc = "the client didn’t respond to a ping event in time"]
        Unresponsive = 6,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::Role),
            1 => Ok(Self::DefunctSurfaces),
            2 => Ok(Self::NotTheTopmostPopup),
            3 => Ok(Self::InvalidPopupParent),
            4 => Ok(Self::InvalidSurfaceState),
            5 => Ok(Self::InvalidPositioner),
            6 => Ok(Self::Unresponsive),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "destroy xdg_wm_base\n\nDestroy this xdg_wm_base object.\n\nDestroying a bound xdg_wm_base object while there are surfaces\nstill alive created by this xdg_wm_base object instance is illegal\nand will result in a defunct_surfaces error."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "create a positioner object\n\nCreate a positioner object. A positioner object is used to position\nsurfaces relative to some parent surface. See the interface description\nand xdg_surface.get_popup for details."]
        CreatePositioner {
        },
    #[doc = "create a shell surface from a surface\n\nThis creates an xdg_surface for the given surface. While xdg_surface\nitself is not a role, the corresponding surface may only be assigned\na role extending xdg_surface, such as xdg_toplevel or xdg_popup. It is\nillegal to create an xdg_surface for a wl_surface which already has an\nassigned role and this will result in a role error.\n\nThis creates an xdg_surface for the given surface. An xdg_surface is\nused as basis to define a role to a given surface, such as xdg_toplevel\nor xdg_popup. It also manages functionality shared between xdg_surface\nbased surface roles.\n\nSee the documentation of xdg_surface for more details about what an\nxdg_surface is and how it is used."]
        GetXdgSurface {
            surface: super::wl_surface::WlSurface,
        },
    #[doc = "respond to a ping event\n\nA client must respond to a ping event with a pong request or\nthe client may be deemed unresponsive. See xdg_wm_base.ping\nand xdg_wm_base.error.unresponsive."]
        Pong {
    #[doc = "serial of the ping event"]
            serial: u32,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::CreatePositioner { .. } => 1,
        Self::GetXdgSurface { .. } => 2,
        Self::Pong { .. } => 3,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_CREATE_POSITIONER_SINCE: u32 = 1;
    pub const REQ_CREATE_POSITIONER_OPCODE: u16 = 1;
    pub const REQ_GET_XDG_SURFACE_SINCE: u32 = 1;
    pub const REQ_GET_XDG_SURFACE_OPCODE: u16 = 2;
    pub const REQ_PONG_SINCE: u32 = 1;
    pub const REQ_PONG_OPCODE: u16 = 3;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    #[doc = "check if the client is alive\n\nThe ping event asks the client if it's still alive. Pass the\nserial specified in the event back to the compositor by sending\na \"pong\" request back with the specified serial. See xdg_wm_base.pong.\n\nCompositors can use this to determine if the client is still\nalive. It's unspecified what will happen if the client doesn't\nrespond to the ping request, or in what timeframe. Clients should\ntry to respond in a reasonable amount of time. The “unresponsive”\nerror is provided for compositors that wish to disconnect unresponsive\nclients.\n\nA compositor is free to ping in any way it wants, but a client must\nalways respond to any xdg_wm_base object it created."]
        Ping {
    #[doc = "pass this to the pong request"]
            serial: u32,
        },
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
        Self::Ping { .. } => 0,
    } } }
    pub const EVT_PING_SINCE: u32 = 1;
    pub const EVT_PING_OPCODE: u16 = 0;

    #[derive(Debug, Clone)]
    pub struct XdgWmBase {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for XdgWmBase { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for XdgWmBase {}
    impl PartialEq<Weak<XdgWmBase>> for XdgWmBase {
        fn eq(&self, other: &Weak<XdgWmBase>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for XdgWmBase { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for XdgWmBase {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for XdgWmBase {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::XDG_WM_BASE_INTERFACE }
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
                if let (Some(Argument::Uint(serial)),) = (arg_iter.next(),) {
                    Ok((me, Event::Ping { serial: serial }))
                } else {
                    Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                }
            }
            _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
        }
    }
    fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
        match request {
            Request::Destroy => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::CreatePositioner {  } => {
                let child_spec = Some((super::xdg_positioner::XdgPositioner::interface(), conn.object_info(self.id())?.version));
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::NewId(ObjectId::null()));
                Ok((msg, child_spec))
            },
            Request::GetXdgSurface { surface } => {
                let child_spec = Some((super::xdg_surface::XdgSurface::interface(), conn.object_info(self.id())?.version));
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::NewId(ObjectId::null()));
                msg.args.push(Argument::Object(surface.id()));
                Ok((msg, child_spec))
            },
            Request::Pong { serial } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                msg.args.push(Argument::Uint(serial));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl XdgWmBase {
    #[doc = "destroy xdg_wm_base\n\nDestroy this xdg_wm_base object.\n\nDestroying a bound xdg_wm_base object while there are surfaces\nstill alive created by this xdg_wm_base object instance is illegal\nand will result in a defunct_surfaces error."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "create a positioner object\n\nCreate a positioner object. A positioner object is used to position\nsurfaces relative to some parent surface. See the interface description\nand xdg_surface.get_popup for details."]
    pub fn create_positioner<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_positioner::XdgPositioner, U> + 'static>(&self, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_positioner::XdgPositioner {
        self.send_constructor::<super::xdg_positioner::XdgPositioner>(Request::CreatePositioner {  }, qh.make_data::<super::xdg_positioner::XdgPositioner, U>(udata)).unwrap_or_else(|_| super::xdg_positioner::XdgPositioner::inert(self.backend.clone()))
    }
    #[doc = "create a shell surface from a surface\n\nThis creates an xdg_surface for the given surface. While xdg_surface\nitself is not a role, the corresponding surface may only be assigned\na role extending xdg_surface, such as xdg_toplevel or xdg_popup. It is\nillegal to create an xdg_surface for a wl_surface which already has an\nassigned role and this will result in a role error.\n\nThis creates an xdg_surface for the given surface. An xdg_surface is\nused as basis to define a role to a given surface, such as xdg_toplevel\nor xdg_popup. It also manages functionality shared between xdg_surface\nbased surface roles.\n\nSee the documentation of xdg_surface for more details about what an\nxdg_surface is and how it is used."]
    pub fn get_xdg_surface<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_surface::XdgSurface, U> + 'static>(&self, surface: &super::wl_surface::WlSurface, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_surface::XdgSurface {
        self.send_constructor::<super::xdg_surface::XdgSurface>(Request::GetXdgSurface { surface: surface.clone() }, qh.make_data::<super::xdg_surface::XdgSurface, U>(udata)).unwrap_or_else(|_| super::xdg_surface::XdgSurface::inert(self.backend.clone()))
    }
    #[doc = "respond to a ping event\n\nA client must respond to a ping event with a pong request or\nthe client may be deemed unresponsive. See xdg_wm_base.ping\nand xdg_wm_base.error.unresponsive."]
    pub fn pong(&self, serial: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Pong { serial: serial }, None);
    }
    }
    }
    #[doc = "child surface positioner\n\nThe xdg_positioner provides a collection of rules for the placement of a\nchild surface relative to a parent surface. Rules can be defined to ensure\nthe child surface remains within the visible area's borders, and to\nspecify how the child surface changes its position, such as sliding along\nan axis, or flipping around a rectangle. These positioner-created rules are\nconstrained by the requirement that a child surface must intersect with or\nbe at least partially adjacent to its parent surface.\n\nSee the various requests for details about possible rules.\n\nAt the time of the request, the compositor makes a copy of the rules\nspecified by the xdg_positioner. Thus, after the request is complete the\nxdg_positioner object can be destroyed or reused; further changes to the\nobject will have no effect on previous usages.\n\nFor an xdg_positioner object to be considered complete, it must have a\nnon-zero size set by set_size, and a non-zero anchor rectangle set by\nset_anchor_rect. Passing an incomplete xdg_positioner object when\npositioning a surface raises an invalid_positioner error."]
    pub mod xdg_positioner {
    use super::wayland_client;
    use std::sync::Arc;
    use std::os::fd::OwnedFd;
    use wayland_client::{Proxy, Connection, DispatchError, Weak};
    use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
    use wayland_client::backend::protocol::Argument;
    use wayland_client::backend::protocol::WEnum;
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Error {
    #[doc = "invalid input provided"]
        InvalidInput = 0,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::InvalidInput),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Anchor {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 3,
        Right = 4,
        TopLeft = 5,
        BottomLeft = 6,
        TopRight = 7,
        BottomRight = 8,
    }
    impl TryFrom<u32> for Anchor {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Top),
            2 => Ok(Self::Bottom),
            3 => Ok(Self::Left),
            4 => Ok(Self::Right),
            5 => Ok(Self::TopLeft),
            6 => Ok(Self::BottomLeft),
            7 => Ok(Self::TopRight),
            8 => Ok(Self::BottomRight),
            _ => Err(()),
        } }
    }
    impl From<Anchor> for u32 { fn from(value: Anchor) -> u32 { value as u32 } }
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Gravity {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 3,
        Right = 4,
        TopLeft = 5,
        BottomLeft = 6,
        TopRight = 7,
        BottomRight = 8,
    }
    impl TryFrom<u32> for Gravity {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Top),
            2 => Ok(Self::Bottom),
            3 => Ok(Self::Left),
            4 => Ok(Self::Right),
            5 => Ok(Self::TopLeft),
            6 => Ok(Self::BottomLeft),
            7 => Ok(Self::TopRight),
            8 => Ok(Self::BottomRight),
            _ => Err(()),
        } }
    }
    impl From<Gravity> for u32 { fn from(value: Gravity) -> u32 { value as u32 } }
    bitflags::bitflags! {
        #[doc = "constraint adjustments\n\nThe constraint adjustment value define ways the compositor will adjust\nthe position of the surface, if the unadjusted position would result\nin the surface being partly constrained.\n\nWhether a surface is considered 'constrained' is left to the compositor\nto determine. For example, the surface may be partly outside the\ncompositor's defined 'work area', thus necessitating the child surface's\nposition be adjusted until it is entirely inside the work area.\n\nThe adjustments can be combined, according to a defined precedence: 1)\nFlip, 2) Slide, 3) Resize."]
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct ConstraintAdjustment: u32 {
            #[doc = "don't move the child surface when constrained\n\nDon't alter the surface position even if it is constrained on some\naxis, for example partially outside the edge of an output."]
            const None = 0;
            #[doc = "move along the x axis until unconstrained\n\nSlide the surface along the x axis until it is no longer constrained.\n\nFirst try to slide towards the direction of the gravity on the x axis\nuntil either the edge in the opposite direction of the gravity is\nunconstrained or the edge in the direction of the gravity is\nconstrained.\n\nThen try to slide towards the opposite direction of the gravity on the\nx axis until either the edge in the direction of the gravity is\nunconstrained or the edge in the opposite direction of the gravity is\nconstrained."]
            const SlideX = 1;
            #[doc = "move along the y axis until unconstrained\n\nSlide the surface along the y axis until it is no longer constrained.\n\nFirst try to slide towards the direction of the gravity on the y axis\nuntil either the edge in the opposite direction of the gravity is\nunconstrained or the edge in the direction of the gravity is\nconstrained.\n\nThen try to slide towards the opposite direction of the gravity on the\ny axis until either the edge in the direction of the gravity is\nunconstrained or the edge in the opposite direction of the gravity is\nconstrained."]
            const SlideY = 2;
            #[doc = "invert the anchor and gravity on the x axis\n\nInvert the anchor and gravity on the x axis if the surface is\nconstrained on the x axis. For example, if the left edge of the\nsurface is constrained, the gravity is 'left' and the anchor is\n'left', change the gravity to 'right' and the anchor to 'right'.\n\nIf the adjusted position also ends up being constrained, the resulting\nposition of the flip_x adjustment will be the one before the\nadjustment."]
            const FlipX = 4;
            #[doc = "invert the anchor and gravity on the y axis\n\nInvert the anchor and gravity on the y axis if the surface is\nconstrained on the y axis. For example, if the bottom edge of the\nsurface is constrained, the gravity is 'bottom' and the anchor is\n'bottom', change the gravity to 'top' and the anchor to 'top'.\n\nThe adjusted position is calculated given the original anchor\nrectangle and offset, but with the new flipped anchor and gravity\nvalues.\n\nIf the adjusted position also ends up being constrained, the resulting\nposition of the flip_y adjustment will be the one before the\nadjustment."]
            const FlipY = 8;
            #[doc = "horizontally resize the surface\n\nResize the surface horizontally so that it is completely\nunconstrained."]
            const ResizeX = 16;
            #[doc = "vertically resize the surface\n\nResize the surface vertically so that it is completely unconstrained."]
            const ResizeY = 32;
        }
    }
    impl TryFrom<u32> for ConstraintAdjustment {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { Self::from_bits(value).ok_or(()) }
    }
    impl From<ConstraintAdjustment> for u32 { fn from(value: ConstraintAdjustment) -> u32 { value.bits() } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "destroy the xdg_positioner object\n\nNotify the compositor that the xdg_positioner will no longer be used."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "set the size of the to-be positioned rectangle\n\nSet the size of the surface that is to be positioned with the positioner\nobject. The size is in surface-local coordinates and corresponds to the\nwindow geometry. See xdg_surface.set_window_geometry.\n\nIf a zero or negative size is set the invalid_input error is raised."]
        SetSize {
    #[doc = "width of positioned rectangle"]
            width: i32,
    #[doc = "height of positioned rectangle"]
            height: i32,
        },
    #[doc = "set the anchor rectangle within the parent surface\n\nSpecify the anchor rectangle within the parent surface that the child\nsurface will be placed relative to. The rectangle is relative to the\nwindow geometry as defined by xdg_surface.set_window_geometry of the\nparent surface.\n\nWhen the xdg_positioner object is used to position a child surface, the\nanchor rectangle may not extend outside the window geometry of the\npositioned child's parent surface.\n\nIf a negative size is set the invalid_input error is raised."]
        SetAnchorRect {
    #[doc = "x position of anchor rectangle"]
            x: i32,
    #[doc = "y position of anchor rectangle"]
            y: i32,
    #[doc = "width of anchor rectangle"]
            width: i32,
    #[doc = "height of anchor rectangle"]
            height: i32,
        },
    #[doc = "set anchor rectangle anchor\n\nDefines the anchor point for the anchor rectangle. The specified anchor\nis used derive an anchor point that the child surface will be\npositioned relative to. If a corner anchor is set (e.g. 'top_left' or\n'bottom_right'), the anchor point will be at the specified corner;\notherwise, the derived anchor point will be centered on the specified\nedge, or in the center of the anchor rectangle if no edge is specified."]
        SetAnchor {
    #[doc = "anchor"]
            anchor: WEnum<Anchor>,
        },
    #[doc = "set child surface gravity\n\nDefines in what direction a surface should be positioned, relative to\nthe anchor point of the parent surface. If a corner gravity is\nspecified (e.g. 'bottom_right' or 'top_left'), then the child surface\nwill be placed towards the specified gravity; otherwise, the child\nsurface will be centered over the anchor point on any axis that had no\ngravity specified. If the gravity is not in the ‘gravity’ enum, an\ninvalid_input error is raised."]
        SetGravity {
    #[doc = "gravity direction"]
            gravity: WEnum<Gravity>,
        },
    #[doc = "set the adjustment to be done when constrained\n\nSpecify how the window should be positioned if the originally intended\nposition caused the surface to be constrained, meaning at least\npartially outside positioning boundaries set by the compositor. The\nadjustment is set by constructing a bitmask describing the adjustment to\nbe made when the surface is constrained on that axis.\n\nIf no bit for one axis is set, the compositor will assume that the child\nsurface should not change its position on that axis when constrained.\n\nIf more than one bit for one axis is set, the order of how adjustments\nare applied is specified in the corresponding adjustment descriptions.\n\nThe default adjustment is none."]
        SetConstraintAdjustment {
    #[doc = "bit mask of constraint adjustments"]
            constraint_adjustment: WEnum<ConstraintAdjustment>,
        },
    #[doc = "set surface position offset\n\nSpecify the surface position offset relative to the position of the\nanchor on the anchor rectangle and the anchor on the surface. For\nexample if the anchor of the anchor rectangle is at (x, y), the surface\nhas the gravity bottom|right, and the offset is (ox, oy), the calculated\nsurface position will be (x + ox, y + oy). The offset position of the\nsurface is the one used for constraint testing. See\nset_constraint_adjustment.\n\nAn example use case is placing a popup menu on top of a user interface\nelement, while aligning the user interface element of the parent surface\nwith some user interface element placed somewhere in the popup surface."]
        SetOffset {
    #[doc = "surface position x offset"]
            x: i32,
    #[doc = "surface position y offset"]
            y: i32,
        },
    #[doc = "continuously reconstrain the surface\n\nWhen set reactive, the surface is reconstrained if the conditions used\nfor constraining changed, e.g. the parent window moved.\n\nIf the conditions changed and the popup was reconstrained, an\nxdg_popup.configure event is sent with updated geometry, followed by an\nxdg_surface.configure event."]
    #[doc = "Available since interface version 3."]
        SetReactive,
    #[doc = "\n\nSet the parent window geometry the compositor should use when\npositioning the popup. The compositor may use this information to\ndetermine the future state the popup should be constrained using. If\nthis doesn't match the dimension of the parent the popup is eventually\npositioned against, the behavior is undefined.\n\nThe arguments are given in the surface-local coordinate space."]
    #[doc = "Available since interface version 3."]
        SetParentSize {
    #[doc = "future window geometry width of parent"]
            parent_width: i32,
    #[doc = "future window geometry height of parent"]
            parent_height: i32,
        },
    #[doc = "set parent configure this is a response to\n\nSet the serial of an xdg_surface.configure event this positioner will be\nused in response to. The compositor may use this information together\nwith set_parent_size to determine what future state the popup should be\nconstrained using."]
    #[doc = "Available since interface version 3."]
        SetParentConfigure {
    #[doc = "serial of parent configure event"]
            serial: u32,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::SetSize { .. } => 1,
        Self::SetAnchorRect { .. } => 2,
        Self::SetAnchor { .. } => 3,
        Self::SetGravity { .. } => 4,
        Self::SetConstraintAdjustment { .. } => 5,
        Self::SetOffset { .. } => 6,
        Self::SetReactive => 7,
        Self::SetParentSize { .. } => 8,
        Self::SetParentConfigure { .. } => 9,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_SET_SIZE_SINCE: u32 = 1;
    pub const REQ_SET_SIZE_OPCODE: u16 = 1;
    pub const REQ_SET_ANCHOR_RECT_SINCE: u32 = 1;
    pub const REQ_SET_ANCHOR_RECT_OPCODE: u16 = 2;
    pub const REQ_SET_ANCHOR_SINCE: u32 = 1;
    pub const REQ_SET_ANCHOR_OPCODE: u16 = 3;
    pub const REQ_SET_GRAVITY_SINCE: u32 = 1;
    pub const REQ_SET_GRAVITY_OPCODE: u16 = 4;
    pub const REQ_SET_CONSTRAINT_ADJUSTMENT_SINCE: u32 = 1;
    pub const REQ_SET_CONSTRAINT_ADJUSTMENT_OPCODE: u16 = 5;
    pub const REQ_SET_OFFSET_SINCE: u32 = 1;
    pub const REQ_SET_OFFSET_OPCODE: u16 = 6;
    pub const REQ_SET_REACTIVE_SINCE: u32 = 3;
    pub const REQ_SET_REACTIVE_OPCODE: u16 = 7;
    pub const REQ_SET_PARENT_SIZE_SINCE: u32 = 3;
    pub const REQ_SET_PARENT_SIZE_OPCODE: u16 = 8;
    pub const REQ_SET_PARENT_CONFIGURE_SINCE: u32 = 3;
    pub const REQ_SET_PARENT_CONFIGURE_OPCODE: u16 = 9;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
    } } }

    #[derive(Debug, Clone)]
    pub struct XdgPositioner {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for XdgPositioner { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for XdgPositioner {}
    impl PartialEq<Weak<XdgPositioner>> for XdgPositioner {
        fn eq(&self, other: &Weak<XdgPositioner>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for XdgPositioner { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for XdgPositioner {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for XdgPositioner {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::XDG_POSITIONER_INTERFACE }
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
            Request::SetSize { width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::SetAnchorRect { x, y, width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::Int(x));
                msg.args.push(Argument::Int(y));
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::SetAnchor { anchor } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                msg.args.push(Argument::Uint(u32::from(anchor)));
                Ok((msg, child_spec))
            },
            Request::SetGravity { gravity } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 4, args: Default::default() };
                msg.args.push(Argument::Uint(u32::from(gravity)));
                Ok((msg, child_spec))
            },
            Request::SetConstraintAdjustment { constraint_adjustment } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 5, args: Default::default() };
                msg.args.push(Argument::Uint(u32::from(constraint_adjustment)));
                Ok((msg, child_spec))
            },
            Request::SetOffset { x, y } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 6, args: Default::default() };
                msg.args.push(Argument::Int(x));
                msg.args.push(Argument::Int(y));
                Ok((msg, child_spec))
            },
            Request::SetReactive => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 7, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::SetParentSize { parent_width, parent_height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 8, args: Default::default() };
                msg.args.push(Argument::Int(parent_width));
                msg.args.push(Argument::Int(parent_height));
                Ok((msg, child_spec))
            },
            Request::SetParentConfigure { serial } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 9, args: Default::default() };
                msg.args.push(Argument::Uint(serial));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl XdgPositioner {
    #[doc = "destroy the xdg_positioner object\n\nNotify the compositor that the xdg_positioner will no longer be used."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "set the size of the to-be positioned rectangle\n\nSet the size of the surface that is to be positioned with the positioner\nobject. The size is in surface-local coordinates and corresponds to the\nwindow geometry. See xdg_surface.set_window_geometry.\n\nIf a zero or negative size is set the invalid_input error is raised."]
    pub fn set_size(&self, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetSize { width: width, height: height }, None);
    }
    #[doc = "set the anchor rectangle within the parent surface\n\nSpecify the anchor rectangle within the parent surface that the child\nsurface will be placed relative to. The rectangle is relative to the\nwindow geometry as defined by xdg_surface.set_window_geometry of the\nparent surface.\n\nWhen the xdg_positioner object is used to position a child surface, the\nanchor rectangle may not extend outside the window geometry of the\npositioned child's parent surface.\n\nIf a negative size is set the invalid_input error is raised."]
    pub fn set_anchor_rect(&self, x: i32, y: i32, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetAnchorRect { x: x, y: y, width: width, height: height }, None);
    }
    #[doc = "set anchor rectangle anchor\n\nDefines the anchor point for the anchor rectangle. The specified anchor\nis used derive an anchor point that the child surface will be\npositioned relative to. If a corner anchor is set (e.g. 'top_left' or\n'bottom_right'), the anchor point will be at the specified corner;\notherwise, the derived anchor point will be centered on the specified\nedge, or in the center of the anchor rectangle if no edge is specified."]
    pub fn set_anchor(&self, anchor: Anchor) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetAnchor { anchor: WEnum::Value(anchor) }, None);
    }
    #[doc = "set child surface gravity\n\nDefines in what direction a surface should be positioned, relative to\nthe anchor point of the parent surface. If a corner gravity is\nspecified (e.g. 'bottom_right' or 'top_left'), then the child surface\nwill be placed towards the specified gravity; otherwise, the child\nsurface will be centered over the anchor point on any axis that had no\ngravity specified. If the gravity is not in the ‘gravity’ enum, an\ninvalid_input error is raised."]
    pub fn set_gravity(&self, gravity: Gravity) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetGravity { gravity: WEnum::Value(gravity) }, None);
    }
    #[doc = "set the adjustment to be done when constrained\n\nSpecify how the window should be positioned if the originally intended\nposition caused the surface to be constrained, meaning at least\npartially outside positioning boundaries set by the compositor. The\nadjustment is set by constructing a bitmask describing the adjustment to\nbe made when the surface is constrained on that axis.\n\nIf no bit for one axis is set, the compositor will assume that the child\nsurface should not change its position on that axis when constrained.\n\nIf more than one bit for one axis is set, the order of how adjustments\nare applied is specified in the corresponding adjustment descriptions.\n\nThe default adjustment is none."]
    pub fn set_constraint_adjustment(&self, constraint_adjustment: ConstraintAdjustment) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetConstraintAdjustment { constraint_adjustment: WEnum::Value(constraint_adjustment) }, None);
    }
    #[doc = "set surface position offset\n\nSpecify the surface position offset relative to the position of the\nanchor on the anchor rectangle and the anchor on the surface. For\nexample if the anchor of the anchor rectangle is at (x, y), the surface\nhas the gravity bottom|right, and the offset is (ox, oy), the calculated\nsurface position will be (x + ox, y + oy). The offset position of the\nsurface is the one used for constraint testing. See\nset_constraint_adjustment.\n\nAn example use case is placing a popup menu on top of a user interface\nelement, while aligning the user interface element of the parent surface\nwith some user interface element placed somewhere in the popup surface."]
    pub fn set_offset(&self, x: i32, y: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetOffset { x: x, y: y }, None);
    }
    #[doc = "continuously reconstrain the surface\n\nWhen set reactive, the surface is reconstrained if the conditions used\nfor constraining changed, e.g. the parent window moved.\n\nIf the conditions changed and the popup was reconstrained, an\nxdg_popup.configure event is sent with updated geometry, followed by an\nxdg_surface.configure event."]
    pub fn set_reactive(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetReactive, None);
    }
    #[doc = "\n\nSet the parent window geometry the compositor should use when\npositioning the popup. The compositor may use this information to\ndetermine the future state the popup should be constrained using. If\nthis doesn't match the dimension of the parent the popup is eventually\npositioned against, the behavior is undefined.\n\nThe arguments are given in the surface-local coordinate space."]
    pub fn set_parent_size(&self, parent_width: i32, parent_height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetParentSize { parent_width: parent_width, parent_height: parent_height }, None);
    }
    #[doc = "set parent configure this is a response to\n\nSet the serial of an xdg_surface.configure event this positioner will be\nused in response to. The compositor may use this information together\nwith set_parent_size to determine what future state the popup should be\nconstrained using."]
    pub fn set_parent_configure(&self, serial: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetParentConfigure { serial: serial }, None);
    }
    }
    }
    #[doc = "desktop user interface surface base interface\n\nAn interface that may be implemented by a wl_surface, for\nimplementations that provide a desktop-style user interface.\n\nIt provides a base set of functionality required to construct user\ninterface elements requiring management by the compositor, such as\ntoplevel windows, menus, etc. The types of functionality are split into\nxdg_surface roles.\n\nCreating an xdg_surface does not set the role for a wl_surface. In order\nto map an xdg_surface, the client must create a role-specific object\nusing, e.g., get_toplevel, get_popup. The wl_surface for any given\nxdg_surface can have at most one role, and may not be assigned any role\nnot based on xdg_surface.\n\nA role must be assigned before any other requests are made to the\nxdg_surface object.\n\nThe client must call wl_surface.commit on the corresponding wl_surface\nfor the xdg_surface state to take effect.\n\nCreating an xdg_surface from a wl_surface which has a buffer attached or\ncommitted is a client error, and any attempts by a client to attach or\nmanipulate a buffer prior to the first xdg_surface.configure call must\nalso be treated as errors.\n\nAfter creating a role-specific object and setting it up (e.g. by sending\nthe title, app ID, size constraints, parent, etc), the client must\nperform an initial commit without any buffer attached. The compositor\nwill reply with initial wl_surface state such as\nwl_surface.preferred_buffer_scale followed by an xdg_surface.configure\nevent. The client must acknowledge it and is then allowed to attach a\nbuffer to map the surface.\n\nMapping an xdg_surface-based role surface is defined as making it\npossible for the surface to be shown by the compositor. Note that\na mapped surface is not guaranteed to be visible once it is mapped.\n\nFor an xdg_surface to be mapped by the compositor, the following\nconditions must be met:\n(1) the client has assigned an xdg_surface-based role to the surface\n(2) the client has set and committed the xdg_surface state and the\nrole-dependent state to the surface\n(3) the client has committed a buffer to the surface\n\nA newly-unmapped surface is considered to have met condition (1) out\nof the 3 required conditions for mapping a surface if its role surface\nhas not been destroyed, i.e. the client must perform the initial commit\nagain before attaching a buffer."]
    pub mod xdg_surface {
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
    #[doc = "Surface was not fully constructed"]
        NotConstructed = 1,
    #[doc = "Surface was already constructed"]
        AlreadyConstructed = 2,
    #[doc = "Attaching a buffer to an unconfigured surface"]
        UnconfiguredBuffer = 3,
    #[doc = "Invalid serial number when acking a configure event"]
        InvalidSerial = 4,
    #[doc = "Width or height was zero or negative"]
        InvalidSize = 5,
    #[doc = "Surface was destroyed before its role object"]
        DefunctRoleObject = 6,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            1 => Ok(Self::NotConstructed),
            2 => Ok(Self::AlreadyConstructed),
            3 => Ok(Self::UnconfiguredBuffer),
            4 => Ok(Self::InvalidSerial),
            5 => Ok(Self::InvalidSize),
            6 => Ok(Self::DefunctRoleObject),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "destroy the xdg_surface\n\nDestroy the xdg_surface object. An xdg_surface must only be destroyed\nafter its role object has been destroyed, otherwise\na defunct_role_object error is raised."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "assign the xdg_toplevel surface role\n\nThis creates an xdg_toplevel object for the given xdg_surface and gives\nthe associated wl_surface the xdg_toplevel role.\n\nSee the documentation of xdg_toplevel for more details about what an\nxdg_toplevel is and how it is used."]
        GetToplevel {
        },
    #[doc = "assign the xdg_popup surface role\n\nThis creates an xdg_popup object for the given xdg_surface and gives\nthe associated wl_surface the xdg_popup role.\n\nIf null is passed as a parent, a parent surface must be specified using\nsome other protocol, before committing the initial state.\n\nSee the documentation of xdg_popup for more details about what an\nxdg_popup is and how it is used."]
        GetPopup {
            parent: Option<super::xdg_surface::XdgSurface>,
            positioner: super::xdg_positioner::XdgPositioner,
        },
    #[doc = "set the new window geometry\n\nThe window geometry of a surface is its \"visible bounds\" from the\nuser's perspective. Client-side decorations often have invisible\nportions like drop-shadows which should be ignored for the\npurposes of aligning, placing and constraining windows. Note that\nin some situations, compositors may clip rendering to the window\ngeometry, so the client should avoid putting functional elements\noutside of it.\n\nThe window geometry is double-buffered state, see wl_surface.commit.\n\nWhen maintaining a position, the compositor should treat the (x, y)\ncoordinate of the window geometry as the top left corner of the window.\nA client changing the (x, y) window geometry coordinate should in\ngeneral not alter the position of the window.\n\nOnce the window geometry of the surface is set, it is not possible to\nunset it, and it will remain the same until set_window_geometry is\ncalled again, even if a new subsurface or buffer is attached.\n\nIf never set, the value is the full bounds of the surface,\nincluding any subsurfaces. This updates dynamically on every\ncommit. This unset is meant for extremely simple clients.\n\nThe arguments are given in the surface-local coordinate space of\nthe wl_surface associated with this xdg_surface, and may extend outside\nof the wl_surface itself to mark parts of the subsurface tree as part of\nthe window geometry.\n\nWhen applied, the effective window geometry will be the set window\ngeometry clamped to the bounding rectangle of the combined\ngeometry of the surface of the xdg_surface and the associated\nsubsurfaces.\n\nThe effective geometry will not be recalculated unless a new call to\nset_window_geometry is done and the new pending surface state is\nsubsequently applied.\n\nThe width and height of the effective window geometry must be\ngreater than zero. Setting an invalid size will raise an\ninvalid_size error."]
        SetWindowGeometry {
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        },
    #[doc = "ack a configure event\n\nWhen a configure event is received, if a client commits the\nsurface in response to the configure event, then the client\nmust make an ack_configure request sometime before the commit\nrequest, passing along the serial of the configure event.\n\nFor instance, for toplevel surfaces the compositor might use this\ninformation to move a surface to the top left only when the client has\ndrawn itself for the maximized or fullscreen state.\n\nIf the client receives multiple configure events before it\ncan respond to one, it only has to ack the last configure event.\nAcking a configure event that was never sent raises an invalid_serial\nerror.\n\nA client is not required to commit immediately after sending\nan ack_configure request - it may even ack_configure several times\nbefore its next surface commit.\n\nA client may send multiple ack_configure requests before committing, but\nonly the last request sent before a commit indicates which configure\nevent the client really is responding to.\n\nSending an ack_configure request consumes the serial number sent with\nthe request, as well as serial numbers sent by all configure events\nsent on this xdg_surface prior to the configure event referenced by\nthe committed serial.\n\nIt is an error to issue multiple ack_configure requests referencing a\nserial from the same configure event, or to issue an ack_configure\nrequest referencing a serial from a configure event issued before the\nevent identified by the last ack_configure request for the same\nxdg_surface. Doing so will raise an invalid_serial error."]
        AckConfigure {
    #[doc = "the serial from the configure event"]
            serial: u32,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::GetToplevel { .. } => 1,
        Self::GetPopup { .. } => 2,
        Self::SetWindowGeometry { .. } => 3,
        Self::AckConfigure { .. } => 4,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_GET_TOPLEVEL_SINCE: u32 = 1;
    pub const REQ_GET_TOPLEVEL_OPCODE: u16 = 1;
    pub const REQ_GET_POPUP_SINCE: u32 = 1;
    pub const REQ_GET_POPUP_OPCODE: u16 = 2;
    pub const REQ_SET_WINDOW_GEOMETRY_SINCE: u32 = 1;
    pub const REQ_SET_WINDOW_GEOMETRY_OPCODE: u16 = 3;
    pub const REQ_ACK_CONFIGURE_SINCE: u32 = 1;
    pub const REQ_ACK_CONFIGURE_OPCODE: u16 = 4;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    #[doc = "suggest a surface change\n\nThe configure event marks the end of a configure sequence. A configure\nsequence is a set of one or more events configuring the state of the\nxdg_surface, including the final xdg_surface.configure event.\n\nWhere applicable, xdg_surface surface roles will during a configure\nsequence extend this event as a latched state sent as events before the\nxdg_surface.configure event. Such events should be considered to make up\na set of atomically applied configuration states, where the\nxdg_surface.configure commits the accumulated state.\n\nClients should arrange their surface for the new states, and then send\nan ack_configure request with the serial sent in this configure event at\nsome point before committing the new surface.\n\nIf the client receives multiple configure events before it can respond\nto one, it is free to discard all but the last event it received."]
        Configure {
    #[doc = "serial of the configure event"]
            serial: u32,
        },
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
        Self::Configure { .. } => 0,
    } } }
    pub const EVT_CONFIGURE_SINCE: u32 = 1;
    pub const EVT_CONFIGURE_OPCODE: u16 = 0;

    #[derive(Debug, Clone)]
    pub struct XdgSurface {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for XdgSurface { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for XdgSurface {}
    impl PartialEq<Weak<XdgSurface>> for XdgSurface {
        fn eq(&self, other: &Weak<XdgSurface>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for XdgSurface { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for XdgSurface {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for XdgSurface {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::XDG_SURFACE_INTERFACE }
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
                if let (Some(Argument::Uint(serial)),) = (arg_iter.next(),) {
                    Ok((me, Event::Configure { serial: serial }))
                } else {
                    Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                }
            }
            _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
        }
    }
    fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
        match request {
            Request::Destroy => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::GetToplevel {  } => {
                let child_spec = Some((super::xdg_toplevel::XdgToplevel::interface(), conn.object_info(self.id())?.version));
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::NewId(ObjectId::null()));
                Ok((msg, child_spec))
            },
            Request::GetPopup { parent, positioner } => {
                let child_spec = Some((super::xdg_popup::XdgPopup::interface(), conn.object_info(self.id())?.version));
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::NewId(ObjectId::null()));
                msg.args.push(Argument::Object(parent.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                msg.args.push(Argument::Object(positioner.id()));
                Ok((msg, child_spec))
            },
            Request::SetWindowGeometry { x, y, width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                msg.args.push(Argument::Int(x));
                msg.args.push(Argument::Int(y));
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::AckConfigure { serial } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 4, args: Default::default() };
                msg.args.push(Argument::Uint(serial));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl XdgSurface {
    #[doc = "destroy the xdg_surface\n\nDestroy the xdg_surface object. An xdg_surface must only be destroyed\nafter its role object has been destroyed, otherwise\na defunct_role_object error is raised."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "assign the xdg_toplevel surface role\n\nThis creates an xdg_toplevel object for the given xdg_surface and gives\nthe associated wl_surface the xdg_toplevel role.\n\nSee the documentation of xdg_toplevel for more details about what an\nxdg_toplevel is and how it is used."]
    pub fn get_toplevel<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_toplevel::XdgToplevel, U> + 'static>(&self, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_toplevel::XdgToplevel {
        self.send_constructor::<super::xdg_toplevel::XdgToplevel>(Request::GetToplevel {  }, qh.make_data::<super::xdg_toplevel::XdgToplevel, U>(udata)).unwrap_or_else(|_| super::xdg_toplevel::XdgToplevel::inert(self.backend.clone()))
    }
    #[doc = "assign the xdg_popup surface role\n\nThis creates an xdg_popup object for the given xdg_surface and gives\nthe associated wl_surface the xdg_popup role.\n\nIf null is passed as a parent, a parent surface must be specified using\nsome other protocol, before committing the initial state.\n\nSee the documentation of xdg_popup for more details about what an\nxdg_popup is and how it is used."]
    pub fn get_popup<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_popup::XdgPopup, U> + 'static>(&self, parent: Option<&super::xdg_surface::XdgSurface>, positioner: &super::xdg_positioner::XdgPositioner, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_popup::XdgPopup {
        self.send_constructor::<super::xdg_popup::XdgPopup>(Request::GetPopup { parent: parent.cloned(), positioner: positioner.clone() }, qh.make_data::<super::xdg_popup::XdgPopup, U>(udata)).unwrap_or_else(|_| super::xdg_popup::XdgPopup::inert(self.backend.clone()))
    }
    #[doc = "set the new window geometry\n\nThe window geometry of a surface is its \"visible bounds\" from the\nuser's perspective. Client-side decorations often have invisible\nportions like drop-shadows which should be ignored for the\npurposes of aligning, placing and constraining windows. Note that\nin some situations, compositors may clip rendering to the window\ngeometry, so the client should avoid putting functional elements\noutside of it.\n\nThe window geometry is double-buffered state, see wl_surface.commit.\n\nWhen maintaining a position, the compositor should treat the (x, y)\ncoordinate of the window geometry as the top left corner of the window.\nA client changing the (x, y) window geometry coordinate should in\ngeneral not alter the position of the window.\n\nOnce the window geometry of the surface is set, it is not possible to\nunset it, and it will remain the same until set_window_geometry is\ncalled again, even if a new subsurface or buffer is attached.\n\nIf never set, the value is the full bounds of the surface,\nincluding any subsurfaces. This updates dynamically on every\ncommit. This unset is meant for extremely simple clients.\n\nThe arguments are given in the surface-local coordinate space of\nthe wl_surface associated with this xdg_surface, and may extend outside\nof the wl_surface itself to mark parts of the subsurface tree as part of\nthe window geometry.\n\nWhen applied, the effective window geometry will be the set window\ngeometry clamped to the bounding rectangle of the combined\ngeometry of the surface of the xdg_surface and the associated\nsubsurfaces.\n\nThe effective geometry will not be recalculated unless a new call to\nset_window_geometry is done and the new pending surface state is\nsubsequently applied.\n\nThe width and height of the effective window geometry must be\ngreater than zero. Setting an invalid size will raise an\ninvalid_size error."]
    pub fn set_window_geometry(&self, x: i32, y: i32, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetWindowGeometry { x: x, y: y, width: width, height: height }, None);
    }
    #[doc = "ack a configure event\n\nWhen a configure event is received, if a client commits the\nsurface in response to the configure event, then the client\nmust make an ack_configure request sometime before the commit\nrequest, passing along the serial of the configure event.\n\nFor instance, for toplevel surfaces the compositor might use this\ninformation to move a surface to the top left only when the client has\ndrawn itself for the maximized or fullscreen state.\n\nIf the client receives multiple configure events before it\ncan respond to one, it only has to ack the last configure event.\nAcking a configure event that was never sent raises an invalid_serial\nerror.\n\nA client is not required to commit immediately after sending\nan ack_configure request - it may even ack_configure several times\nbefore its next surface commit.\n\nA client may send multiple ack_configure requests before committing, but\nonly the last request sent before a commit indicates which configure\nevent the client really is responding to.\n\nSending an ack_configure request consumes the serial number sent with\nthe request, as well as serial numbers sent by all configure events\nsent on this xdg_surface prior to the configure event referenced by\nthe committed serial.\n\nIt is an error to issue multiple ack_configure requests referencing a\nserial from the same configure event, or to issue an ack_configure\nrequest referencing a serial from a configure event issued before the\nevent identified by the last ack_configure request for the same\nxdg_surface. Doing so will raise an invalid_serial error."]
    pub fn ack_configure(&self, serial: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::AckConfigure { serial: serial }, None);
    }
    }
    }
    #[doc = "toplevel surface\n\nThis interface defines an xdg_surface role which allows a surface to,\namong other things, set window-like properties such as maximize,\nfullscreen, and minimize, set application-specific metadata like title and\nid, and well as trigger user interactive operations such as interactive\nresize and move.\n\nA xdg_toplevel by default is responsible for providing the full intended\nvisual representation of the toplevel, which depending on the window\nstate, may mean things like a title bar, window controls and drop shadow.\n\nUnmapping an xdg_toplevel means that the surface cannot be shown\nby the compositor until it is explicitly mapped again.\nAll active operations (e.g., move, resize) are canceled and all\nattributes (e.g. title, state, stacking, ...) are discarded for\nan xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to\nthe state it had right after xdg_surface.get_toplevel. The client\ncan re-map the toplevel by performing a commit without any buffer\nattached, waiting for a configure event and handling it as usual (see\nxdg_surface description).\n\nAttaching a null buffer to a toplevel unmaps the surface."]
    pub mod xdg_toplevel {
    use super::wayland_client;
    use std::sync::Arc;
    use std::os::fd::OwnedFd;
    use wayland_client::{Proxy, Connection, DispatchError, Weak};
    use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
    use wayland_client::backend::protocol::Argument;
    use wayland_client::backend::protocol::WEnum;
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum Error {
    #[doc = "provided value is\n        not a valid variant of the resize_edge enum"]
        InvalidResizeEdge = 0,
    #[doc = "invalid parent toplevel"]
        InvalidParent = 1,
    #[doc = "client provided an invalid min or max size"]
        InvalidSize = 2,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::InvalidResizeEdge),
            1 => Ok(Self::InvalidParent),
            2 => Ok(Self::InvalidSize),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[doc = "edge values for resizing\n\nThese values are used to indicate which edge of a surface\nis being dragged in a resize operation."]
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum ResizeEdge {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 4,
        TopLeft = 5,
        BottomLeft = 6,
        Right = 8,
        TopRight = 9,
        BottomRight = 10,
    }
    impl TryFrom<u32> for ResizeEdge {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Top),
            2 => Ok(Self::Bottom),
            4 => Ok(Self::Left),
            5 => Ok(Self::TopLeft),
            6 => Ok(Self::BottomLeft),
            8 => Ok(Self::Right),
            9 => Ok(Self::TopRight),
            10 => Ok(Self::BottomRight),
            _ => Err(()),
        } }
    }
    impl From<ResizeEdge> for u32 { fn from(value: ResizeEdge) -> u32 { value as u32 } }
    #[doc = "types of state on the surface\n\nThe different state values used on the surface. This is designed for\nstate values like maximized, fullscreen. It is paired with the\nconfigure event to ensure that both the client and the compositor\nsetting the state can be synchronized.\n\nStates set in this way are double-buffered, see wl_surface.commit."]
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum State {
    #[doc = "the surface is maximized\n\nThe surface is maximized. The window geometry specified in the configure\nevent must be obeyed by the client, or the xdg_wm_base.invalid_surface_state\nerror is raised.\n\nThe client should draw without shadow or other\ndecoration outside of the window geometry."]
        Maximized = 1,
    #[doc = "the surface is fullscreen\n\nThe surface is fullscreen. The window geometry specified in the\nconfigure event is a maximum; the client cannot resize beyond it. For\na surface to cover the whole fullscreened area, the geometry\ndimensions must be obeyed by the client. For more details, see\nxdg_toplevel.set_fullscreen."]
        Fullscreen = 2,
    #[doc = "the surface is being resized\n\nThe surface is being resized. The window geometry specified in the\nconfigure event is a maximum; the client cannot resize beyond it.\nClients that have aspect ratio or cell sizing configuration can use\na smaller size, however."]
        Resizing = 3,
    #[doc = "the surface is now activated\n\nClient window decorations should be painted as if the window is\nactive. Do not assume this means that the window actually has\nkeyboard or pointer focus."]
        Activated = 4,
    #[doc = "the surface’s left edge is tiled\n\nThe window is currently in a tiled layout and the left edge is\nconsidered to be adjacent to another part of the tiling grid.\n\nThe client should draw without shadow or other decoration outside of\nthe window geometry on the left edge."]
        TiledLeft = 5,
    #[doc = "the surface’s right edge is tiled\n\nThe window is currently in a tiled layout and the right edge is\nconsidered to be adjacent to another part of the tiling grid.\n\nThe client should draw without shadow or other decoration outside of\nthe window geometry on the right edge."]
        TiledRight = 6,
    #[doc = "the surface’s top edge is tiled\n\nThe window is currently in a tiled layout and the top edge is\nconsidered to be adjacent to another part of the tiling grid.\n\nThe client should draw without shadow or other decoration outside of\nthe window geometry on the top edge."]
        TiledTop = 7,
    #[doc = "the surface’s bottom edge is tiled\n\nThe window is currently in a tiled layout and the bottom edge is\nconsidered to be adjacent to another part of the tiling grid.\n\nThe client should draw without shadow or other decoration outside of\nthe window geometry on the bottom edge."]
        TiledBottom = 8,
    #[doc = "surface repaint is suspended\n\nThe surface is currently not ordinarily being repainted; for\nexample because its content is occluded by another window, or its\noutputs are switched off due to screen locking."]
        Suspended = 9,
    #[doc = "the surface’s left edge is constrained\n\nThe left edge of the window is currently constrained, meaning it\nshouldn't attempt to resize from that edge. It can for example mean\nit's tiled next to a monitor edge on the constrained side of the\nwindow."]
        ConstrainedLeft = 10,
    #[doc = "the surface’s right edge is constrained\n\nThe right edge of the window is currently constrained, meaning it\nshouldn't attempt to resize from that edge. It can for example mean\nit's tiled next to a monitor edge on the constrained side of the\nwindow."]
        ConstrainedRight = 11,
    #[doc = "the surface’s top edge is constrained\n\nThe top edge of the window is currently constrained, meaning it\nshouldn't attempt to resize from that edge. It can for example mean\nit's tiled next to a monitor edge on the constrained side of the\nwindow."]
        ConstrainedTop = 12,
    #[doc = "the surface’s bottom edge is constrained\n\nThe bottom edge of the window is currently constrained, meaning it\nshouldn't attempt to resize from that edge. It can for example mean\nit's tiled next to a monitor edge on the constrained side of the\nwindow."]
        ConstrainedBottom = 13,
    }
    impl TryFrom<u32> for State {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            1 => Ok(Self::Maximized),
            2 => Ok(Self::Fullscreen),
            3 => Ok(Self::Resizing),
            4 => Ok(Self::Activated),
            5 => Ok(Self::TiledLeft),
            6 => Ok(Self::TiledRight),
            7 => Ok(Self::TiledTop),
            8 => Ok(Self::TiledBottom),
            9 => Ok(Self::Suspended),
            10 => Ok(Self::ConstrainedLeft),
            11 => Ok(Self::ConstrainedRight),
            12 => Ok(Self::ConstrainedTop),
            13 => Ok(Self::ConstrainedBottom),
            _ => Err(()),
        } }
    }
    impl From<State> for u32 { fn from(value: State) -> u32 { value as u32 } }
    #[repr(u32)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[non_exhaustive]
    pub enum WmCapabilities {
    #[doc = "show_window_menu is available"]
        WindowMenu = 1,
    #[doc = "set_maximized and unset_maximized are available"]
        Maximize = 2,
    #[doc = "set_fullscreen and unset_fullscreen are available"]
        Fullscreen = 3,
    #[doc = "set_minimized is available"]
        Minimize = 4,
    }
    impl TryFrom<u32> for WmCapabilities {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            1 => Ok(Self::WindowMenu),
            2 => Ok(Self::Maximize),
            3 => Ok(Self::Fullscreen),
            4 => Ok(Self::Minimize),
            _ => Err(()),
        } }
    }
    impl From<WmCapabilities> for u32 { fn from(value: WmCapabilities) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "destroy the xdg_toplevel\n\nThis request destroys the role surface and unmaps the surface;\nsee \"Unmapping\" behavior in interface section for details."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "set the parent of this surface\n\nSet the \"parent\" of this surface. This surface should be stacked\nabove the parent surface and all other ancestor surfaces.\n\nParent surfaces should be set on dialogs, toolboxes, or other\n\"auxiliary\" surfaces, so that the parent is raised when the dialog\nis raised.\n\nSetting a null parent for a child surface unsets its parent. Setting\na null parent for a surface which currently has no parent is a no-op.\n\nOnly mapped surfaces can have child surfaces. Setting a parent which\nis not mapped is equivalent to setting a null parent. If a surface\nbecomes unmapped, its children's parent is set to the parent of\nthe now-unmapped surface. If the now-unmapped surface has no parent,\nits children's parent is unset. If the now-unmapped surface becomes\nmapped again, its parent-child relationship is not restored.\n\nThe parent toplevel must not be one of the child toplevel's\ndescendants, and the parent must be different from the child toplevel,\notherwise the invalid_parent protocol error is raised."]
        SetParent {
            parent: Option<super::xdg_toplevel::XdgToplevel>,
        },
    #[doc = "set surface title\n\nSet a short title for the surface.\n\nThis string may be used to identify the surface in a task bar,\nwindow list, or other user interface elements provided by the\ncompositor.\n\nThe string must be encoded in UTF-8."]
        SetTitle {
            title: String,
        },
    #[doc = "set application ID\n\nSet an application identifier for the surface.\n\nThe app ID identifies the general class of applications to which\nthe surface belongs. The compositor can use this to group multiple\nsurfaces together, or to determine how to launch a new application.\n\nFor D-Bus activatable applications, the app ID is used as the D-Bus\nservice name.\n\nThe compositor shell will try to group application surfaces together\nby their app ID. As a best practice, it is suggested to select app\nID's that match the basename of the application's .desktop file.\nFor example, \"org.freedesktop.FooViewer\" where the .desktop file is\n\"org.freedesktop.FooViewer.desktop\".\n\nLike other properties, a set_app_id request can be sent after the\nxdg_toplevel has been mapped to update the property.\n\nSee the desktop-entry specification [0] for more details on\napplication identifiers and how they relate to well-known D-Bus\nnames and .desktop files.\n\n[0] https://standards.freedesktop.org/desktop-entry-spec/"]
        SetAppId {
            app_id: String,
        },
    #[doc = "show the window menu\n\nClients implementing client-side decorations might want to show\na context menu when right-clicking on the decorations, giving the\nuser a menu that they can use to maximize or minimize the window.\n\nThis request asks the compositor to pop up such a window menu at\nthe given position, relative to the local surface coordinates of\nthe parent surface. There are no guarantees as to what menu items\nthe window menu contains, or even if a window menu will be drawn\nat all.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event."]
        ShowWindowMenu {
    #[doc = "the wl_seat of the user event"]
            seat: super::wl_seat::WlSeat,
    #[doc = "the serial of the user event"]
            serial: u32,
    #[doc = "the x position to pop up the window menu at"]
            x: i32,
    #[doc = "the y position to pop up the window menu at"]
            y: i32,
        },
    #[doc = "start an interactive move\n\nStart an interactive, user-driven move of the surface.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event. The passed\nserial is used to determine the type of interactive move (touch,\npointer, etc).\n\nThe server may ignore move requests depending on the state of\nthe surface (e.g. fullscreen or maximized), or if the passed serial\nis no longer valid.\n\nIf triggered, the surface will lose the focus of the device\n(wl_pointer, wl_touch, etc) used for the move. It is up to the\ncompositor to visually indicate that the move is taking place, such as\nupdating a pointer cursor, during the move. There is no guarantee\nthat the device focus will return when the move is completed."]
        Move {
    #[doc = "the wl_seat of the user event"]
            seat: super::wl_seat::WlSeat,
    #[doc = "the serial of the user event"]
            serial: u32,
        },
    #[doc = "start an interactive resize\n\nStart a user-driven, interactive resize of the surface.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event. The passed\nserial is used to determine the type of interactive resize (touch,\npointer, etc).\n\nThe server may ignore resize requests depending on the state of\nthe surface (e.g. fullscreen or maximized).\n\nIf triggered, the client will receive configure events with the\n\"resize\" state enum value and the expected sizes. See the \"resize\"\nenum value for more details about what is required. The client\nmust also acknowledge configure events using \"ack_configure\". After\nthe resize is completed, the client will receive another \"configure\"\nevent without the resize state.\n\nIf triggered, the surface also will lose the focus of the device\n(wl_pointer, wl_touch, etc) used for the resize. It is up to the\ncompositor to visually indicate that the resize is taking place,\nsuch as updating a pointer cursor, during the resize. There is no\nguarantee that the device focus will return when the resize is\ncompleted.\n\nThe edges parameter specifies how the surface should be resized, and\nis one of the values of the resize_edge enum. Values not matching\na variant of the enum will cause the invalid_resize_edge protocol error.\nThe compositor may use this information to update the surface position\nfor example when dragging the top left corner. The compositor may also\nuse this information to adapt its behavior, e.g. choose an appropriate\ncursor image."]
        Resize {
    #[doc = "the wl_seat of the user event"]
            seat: super::wl_seat::WlSeat,
    #[doc = "the serial of the user event"]
            serial: u32,
    #[doc = "which edge or corner is being dragged"]
            edges: WEnum<ResizeEdge>,
        },
    #[doc = "set the maximum size\n\nSet a maximum size for the window.\n\nThe client can specify a maximum size so that the compositor does\nnot try to configure the window beyond this size.\n\nThe width and height arguments are in window geometry coordinates.\nSee xdg_surface.set_window_geometry.\n\nValues set in this way are double-buffered, see wl_surface.commit.\n\nThe compositor can use this information to allow or disallow\ndifferent states like maximize or fullscreen and draw accurate\nanimations.\n\nSimilarly, a tiling window manager may use this information to\nplace and resize client windows in a more effective way.\n\nThe client should not rely on the compositor to obey the maximum\nsize. The compositor may decide to ignore the values set by the\nclient and request a larger size.\n\nIf never set, or a value of zero in the request, means that the\nclient has no expected maximum size in the given dimension.\nAs a result, a client wishing to reset the maximum size\nto an unspecified state can use zero for width and height in the\nrequest.\n\nRequesting a maximum size to be smaller than the minimum size of\na surface is illegal and will result in an invalid_size error.\n\nThe width and height must be greater than or equal to zero. Using\nstrictly negative values for width or height will result in a\ninvalid_size error."]
        SetMaxSize {
            width: i32,
            height: i32,
        },
    #[doc = "set the minimum size\n\nSet a minimum size for the window.\n\nThe client can specify a minimum size so that the compositor does\nnot try to configure the window below this size.\n\nThe width and height arguments are in window geometry coordinates.\nSee xdg_surface.set_window_geometry.\n\nValues set in this way are double-buffered, see wl_surface.commit.\n\nThe compositor can use this information to allow or disallow\ndifferent states like maximize or fullscreen and draw accurate\nanimations.\n\nSimilarly, a tiling window manager may use this information to\nplace and resize client windows in a more effective way.\n\nThe client should not rely on the compositor to obey the minimum\nsize. The compositor may decide to ignore the values set by the\nclient and request a smaller size.\n\nIf never set, or a value of zero in the request, means that the\nclient has no expected minimum size in the given dimension.\nAs a result, a client wishing to reset the minimum size\nto an unspecified state can use zero for width and height in the\nrequest.\n\nRequesting a minimum size to be larger than the maximum size of\na surface is illegal and will result in an invalid_size error.\n\nThe width and height must be greater than or equal to zero. Using\nstrictly negative values for width and height will result in a\ninvalid_size error."]
        SetMinSize {
            width: i32,
            height: i32,
        },
    #[doc = "maximize the window\n\nMaximize the surface.\n\nAfter requesting that the surface should be maximized, the compositor\nwill respond by emitting a configure event. Whether this configure\nactually sets the window maximized is subject to compositor policies.\nThe client must then update its content, drawing in the configured\nstate. The client must also acknowledge the configure when committing\nthe new content (see ack_configure).\n\nIt is up to the compositor to decide how and where to maximize the\nsurface, for example which output and what region of the screen should\nbe used.\n\nIf the surface was already maximized, the compositor will still emit\na configure event with the \"maximized\" state.\n\nIf the surface is in a fullscreen state, this request has no direct\neffect. It may alter the state the surface is returned to when\nunmaximized unless overridden by the compositor."]
        SetMaximized,
    #[doc = "unmaximize the window\n\nUnmaximize the surface.\n\nAfter requesting that the surface should be unmaximized, the compositor\nwill respond by emitting a configure event. Whether this actually\nun-maximizes the window is subject to compositor policies.\nIf available and applicable, the compositor will include the window\ngeometry dimensions the window had prior to being maximized in the\nconfigure event. The client must then update its content, drawing it in\nthe configured state. The client must also acknowledge the configure\nwhen committing the new content (see ack_configure).\n\nIt is up to the compositor to position the surface after it was\nunmaximized; usually the position the surface had before maximizing, if\napplicable.\n\nIf the surface was already not maximized, the compositor will still\nemit a configure event without the \"maximized\" state.\n\nIf the surface is in a fullscreen state, this request has no direct\neffect. It may alter the state the surface is returned to when\nunmaximized unless overridden by the compositor."]
        UnsetMaximized,
    #[doc = "set the window as fullscreen on an output\n\nMake the surface fullscreen.\n\nAfter requesting that the surface should be fullscreened, the\ncompositor will respond by emitting a configure event. Whether the\nclient is actually put into a fullscreen state is subject to compositor\npolicies. The client must also acknowledge the configure when\ncommitting the new content (see ack_configure).\n\nThe output passed by the request indicates the client's preference as\nto which display it should be set fullscreen on. If this value is NULL,\nit's up to the compositor to choose which display will be used to map\nthis surface.\n\nIf the surface doesn't cover the whole output, the compositor will\nposition the surface in the center of the output and compensate with\nwith border fill covering the rest of the output. The content of the\nborder fill is undefined, but should be assumed to be in some way that\nattempts to blend into the surrounding area (e.g. solid black).\n\nIf the fullscreened surface is not opaque, the compositor must make\nsure that other screen content not part of the same surface tree (made\nup of subsurfaces, popups or similarly coupled surfaces) are not\nvisible below the fullscreened surface."]
        SetFullscreen {
            output: Option<super::wl_output::WlOutput>,
        },
    #[doc = "unset the window as fullscreen\n\nMake the surface no longer fullscreen.\n\nAfter requesting that the surface should be unfullscreened, the\ncompositor will respond by emitting a configure event.\nWhether this actually removes the fullscreen state of the client is\nsubject to compositor policies.\n\nMaking a surface unfullscreen sets states for the surface based on the following:\n* the state(s) it may have had before becoming fullscreen\n* any state(s) decided by the compositor\n* any state(s) requested by the client while the surface was fullscreen\n\nThe compositor may include the previous window geometry dimensions in\nthe configure event, if applicable.\n\nThe client must also acknowledge the configure when committing the new\ncontent (see ack_configure)."]
        UnsetFullscreen,
    #[doc = "set the window as minimized\n\nRequest that the compositor minimize your surface. There is no\nway to know if the surface is currently minimized, nor is there\nany way to unset minimization on this surface.\n\nIf you are looking to throttle redrawing when minimized, please\ninstead use the wl_surface.frame event for this, as this will\nalso work with live previews on windows in Alt-Tab, Expose or\nsimilar compositor features."]
        SetMinimized,
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::SetParent { .. } => 1,
        Self::SetTitle { .. } => 2,
        Self::SetAppId { .. } => 3,
        Self::ShowWindowMenu { .. } => 4,
        Self::Move { .. } => 5,
        Self::Resize { .. } => 6,
        Self::SetMaxSize { .. } => 7,
        Self::SetMinSize { .. } => 8,
        Self::SetMaximized => 9,
        Self::UnsetMaximized => 10,
        Self::SetFullscreen { .. } => 11,
        Self::UnsetFullscreen => 12,
        Self::SetMinimized => 13,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_SET_PARENT_SINCE: u32 = 1;
    pub const REQ_SET_PARENT_OPCODE: u16 = 1;
    pub const REQ_SET_TITLE_SINCE: u32 = 1;
    pub const REQ_SET_TITLE_OPCODE: u16 = 2;
    pub const REQ_SET_APP_ID_SINCE: u32 = 1;
    pub const REQ_SET_APP_ID_OPCODE: u16 = 3;
    pub const REQ_SHOW_WINDOW_MENU_SINCE: u32 = 1;
    pub const REQ_SHOW_WINDOW_MENU_OPCODE: u16 = 4;
    pub const REQ_MOVE_SINCE: u32 = 1;
    pub const REQ_MOVE_OPCODE: u16 = 5;
    pub const REQ_RESIZE_SINCE: u32 = 1;
    pub const REQ_RESIZE_OPCODE: u16 = 6;
    pub const REQ_SET_MAX_SIZE_SINCE: u32 = 1;
    pub const REQ_SET_MAX_SIZE_OPCODE: u16 = 7;
    pub const REQ_SET_MIN_SIZE_SINCE: u32 = 1;
    pub const REQ_SET_MIN_SIZE_OPCODE: u16 = 8;
    pub const REQ_SET_MAXIMIZED_SINCE: u32 = 1;
    pub const REQ_SET_MAXIMIZED_OPCODE: u16 = 9;
    pub const REQ_UNSET_MAXIMIZED_SINCE: u32 = 1;
    pub const REQ_UNSET_MAXIMIZED_OPCODE: u16 = 10;
    pub const REQ_SET_FULLSCREEN_SINCE: u32 = 1;
    pub const REQ_SET_FULLSCREEN_OPCODE: u16 = 11;
    pub const REQ_UNSET_FULLSCREEN_SINCE: u32 = 1;
    pub const REQ_UNSET_FULLSCREEN_OPCODE: u16 = 12;
    pub const REQ_SET_MINIMIZED_SINCE: u32 = 1;
    pub const REQ_SET_MINIMIZED_OPCODE: u16 = 13;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    #[doc = "suggest a surface change\n\nThis configure event asks the client to resize its toplevel surface or\nto change its state. The configured state should not be applied\nimmediately. See xdg_surface.configure for details.\n\nThe width and height arguments specify a hint to the window\nabout how its surface should be resized in window geometry\ncoordinates. See set_window_geometry.\n\nIf the width or height arguments are zero, it means the client\nshould decide its own window dimension. This may happen when the\ncompositor needs to configure the state of the surface but doesn't\nhave any information about any previous or expected dimension.\n\nThe states listed in the event specify how the width/height\narguments should be interpreted, and possibly how it should be\ndrawn.\n\nClients must send an ack_configure in response to this event. See\nxdg_surface.configure and xdg_surface.ack_configure for details."]
        Configure {
            width: i32,
            height: i32,
            states: Vec<u8>,
        },
    #[doc = "surface wants to be closed\n\nThe close event is sent by the compositor when the user\nwants the surface to be closed. This should be equivalent to\nthe user clicking the close button in client-side decorations,\nif your application has any.\n\nThis is only a request that the user intends to close the\nwindow. The client may choose to ignore this request, or show\na dialog to ask the user to save their data, etc."]
        Close,
    #[doc = "recommended window geometry bounds\n\nThe configure_bounds event may be sent prior to a xdg_toplevel.configure\nevent to communicate the bounds a window geometry size is recommended\nto constrain to.\n\nThe passed width and height are in surface coordinate space. If width\nand height are 0, it means bounds is unknown and equivalent to as if no\nconfigure_bounds event was ever sent for this surface.\n\nThe bounds can for example correspond to the size of a monitor excluding\nany panels or other shell components, so that a surface isn't created in\na way that it cannot fit.\n\nThe bounds may change at any point, and in such a case, a new\nxdg_toplevel.configure_bounds will be sent, followed by\nxdg_toplevel.configure and xdg_surface.configure."]
    #[doc = "Available since interface version 4."]
        ConfigureBounds {
            width: i32,
            height: i32,
        },
    #[doc = "compositor capabilities\n\nThis event advertises the capabilities supported by the compositor. If\na capability isn't supported, clients should hide or disable the UI\nelements that expose this functionality. For instance, if the\ncompositor doesn't advertise support for minimized toplevels, a button\ntriggering the set_minimized request should not be displayed.\n\nThe compositor will ignore requests it doesn't support. For instance,\na compositor which doesn't advertise support for minimized will ignore\nset_minimized requests.\n\nCompositors must send this event once before the first\nxdg_surface.configure event. When the capabilities change, compositors\nmust send this event again and then send an xdg_surface.configure\nevent.\n\nThe configured state should not be applied immediately. See\nxdg_surface.configure for details.\n\nThe capabilities are sent as an array of 32-bit unsigned integers in\nnative endianness."]
    #[doc = "Available since interface version 5."]
        WmCapabilities {
    #[doc = "array of 32-bit capabilities"]
            capabilities: Vec<u8>,
        },
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
        Self::Configure { .. } => 0,
        Self::Close => 1,
        Self::ConfigureBounds { .. } => 2,
        Self::WmCapabilities { .. } => 3,
    } } }
    pub const EVT_CONFIGURE_SINCE: u32 = 1;
    pub const EVT_CONFIGURE_OPCODE: u16 = 0;
    pub const EVT_CLOSE_SINCE: u32 = 1;
    pub const EVT_CLOSE_OPCODE: u16 = 1;
    pub const EVT_CONFIGURE_BOUNDS_SINCE: u32 = 4;
    pub const EVT_CONFIGURE_BOUNDS_OPCODE: u16 = 2;
    pub const EVT_WM_CAPABILITIES_SINCE: u32 = 5;
    pub const EVT_WM_CAPABILITIES_OPCODE: u16 = 3;

    #[derive(Debug, Clone)]
    pub struct XdgToplevel {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for XdgToplevel { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for XdgToplevel {}
    impl PartialEq<Weak<XdgToplevel>> for XdgToplevel {
        fn eq(&self, other: &Weak<XdgToplevel>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for XdgToplevel { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for XdgToplevel {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for XdgToplevel {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::XDG_TOPLEVEL_INTERFACE }
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
                if let (Some(Argument::Int(width)), Some(Argument::Int(height)), Some(Argument::Array(states)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                    Ok((me, Event::Configure { width: width, height: height, states: *states }))
                } else {
                    Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                }
            }
            1 => Ok((me, Event::Close)),
            2 => {
                if let (Some(Argument::Int(width)), Some(Argument::Int(height)),) = (arg_iter.next(), arg_iter.next(),) {
                    Ok((me, Event::ConfigureBounds { width: width, height: height }))
                } else {
                    Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                }
            }
            3 => {
                if let (Some(Argument::Array(capabilities)),) = (arg_iter.next(),) {
                    Ok((me, Event::WmCapabilities { capabilities: *capabilities }))
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
            Request::SetParent { parent } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::Object(parent.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                Ok((msg, child_spec))
            },
            Request::SetTitle { title } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(title).unwrap()))));
                Ok((msg, child_spec))
            },
            Request::SetAppId { app_id } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(app_id).unwrap()))));
                Ok((msg, child_spec))
            },
            Request::ShowWindowMenu { seat, serial, x, y } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 4, args: Default::default() };
                msg.args.push(Argument::Object(seat.id()));
                msg.args.push(Argument::Uint(serial));
                msg.args.push(Argument::Int(x));
                msg.args.push(Argument::Int(y));
                Ok((msg, child_spec))
            },
            Request::Move { seat, serial } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 5, args: Default::default() };
                msg.args.push(Argument::Object(seat.id()));
                msg.args.push(Argument::Uint(serial));
                Ok((msg, child_spec))
            },
            Request::Resize { seat, serial, edges } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 6, args: Default::default() };
                msg.args.push(Argument::Object(seat.id()));
                msg.args.push(Argument::Uint(serial));
                msg.args.push(Argument::Uint(u32::from(edges)));
                Ok((msg, child_spec))
            },
            Request::SetMaxSize { width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 7, args: Default::default() };
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::SetMinSize { width, height } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 8, args: Default::default() };
                msg.args.push(Argument::Int(width));
                msg.args.push(Argument::Int(height));
                Ok((msg, child_spec))
            },
            Request::SetMaximized => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 9, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::UnsetMaximized => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 10, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::SetFullscreen { output } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 11, args: Default::default() };
                msg.args.push(Argument::Object(output.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                Ok((msg, child_spec))
            },
            Request::UnsetFullscreen => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 12, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::SetMinimized => {
                let child_spec = None;
                let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 13, args: Default::default() };
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl XdgToplevel {
    #[doc = "destroy the xdg_toplevel\n\nThis request destroys the role surface and unmaps the surface;\nsee \"Unmapping\" behavior in interface section for details."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "set the parent of this surface\n\nSet the \"parent\" of this surface. This surface should be stacked\nabove the parent surface and all other ancestor surfaces.\n\nParent surfaces should be set on dialogs, toolboxes, or other\n\"auxiliary\" surfaces, so that the parent is raised when the dialog\nis raised.\n\nSetting a null parent for a child surface unsets its parent. Setting\na null parent for a surface which currently has no parent is a no-op.\n\nOnly mapped surfaces can have child surfaces. Setting a parent which\nis not mapped is equivalent to setting a null parent. If a surface\nbecomes unmapped, its children's parent is set to the parent of\nthe now-unmapped surface. If the now-unmapped surface has no parent,\nits children's parent is unset. If the now-unmapped surface becomes\nmapped again, its parent-child relationship is not restored.\n\nThe parent toplevel must not be one of the child toplevel's\ndescendants, and the parent must be different from the child toplevel,\notherwise the invalid_parent protocol error is raised."]
    pub fn set_parent(&self, parent: Option<&super::xdg_toplevel::XdgToplevel>) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetParent { parent: parent.cloned() }, None);
    }
    #[doc = "set surface title\n\nSet a short title for the surface.\n\nThis string may be used to identify the surface in a task bar,\nwindow list, or other user interface elements provided by the\ncompositor.\n\nThe string must be encoded in UTF-8."]
    pub fn set_title(&self, title: String) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetTitle { title: title }, None);
    }
    #[doc = "set application ID\n\nSet an application identifier for the surface.\n\nThe app ID identifies the general class of applications to which\nthe surface belongs. The compositor can use this to group multiple\nsurfaces together, or to determine how to launch a new application.\n\nFor D-Bus activatable applications, the app ID is used as the D-Bus\nservice name.\n\nThe compositor shell will try to group application surfaces together\nby their app ID. As a best practice, it is suggested to select app\nID's that match the basename of the application's .desktop file.\nFor example, \"org.freedesktop.FooViewer\" where the .desktop file is\n\"org.freedesktop.FooViewer.desktop\".\n\nLike other properties, a set_app_id request can be sent after the\nxdg_toplevel has been mapped to update the property.\n\nSee the desktop-entry specification [0] for more details on\napplication identifiers and how they relate to well-known D-Bus\nnames and .desktop files.\n\n[0] https://standards.freedesktop.org/desktop-entry-spec/"]
    pub fn set_app_id(&self, app_id: String) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetAppId { app_id: app_id }, None);
    }
    #[doc = "show the window menu\n\nClients implementing client-side decorations might want to show\na context menu when right-clicking on the decorations, giving the\nuser a menu that they can use to maximize or minimize the window.\n\nThis request asks the compositor to pop up such a window menu at\nthe given position, relative to the local surface coordinates of\nthe parent surface. There are no guarantees as to what menu items\nthe window menu contains, or even if a window menu will be drawn\nat all.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event."]
    pub fn show_window_menu(&self, seat: &super::wl_seat::WlSeat, serial: u32, x: i32, y: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::ShowWindowMenu { seat: seat.clone(), serial: serial, x: x, y: y }, None);
    }
    #[doc = "start an interactive move\n\nStart an interactive, user-driven move of the surface.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event. The passed\nserial is used to determine the type of interactive move (touch,\npointer, etc).\n\nThe server may ignore move requests depending on the state of\nthe surface (e.g. fullscreen or maximized), or if the passed serial\nis no longer valid.\n\nIf triggered, the surface will lose the focus of the device\n(wl_pointer, wl_touch, etc) used for the move. It is up to the\ncompositor to visually indicate that the move is taking place, such as\nupdating a pointer cursor, during the move. There is no guarantee\nthat the device focus will return when the move is completed."]
    pub fn _move(&self, seat: &super::wl_seat::WlSeat, serial: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Move { seat: seat.clone(), serial: serial }, None);
    }
    #[doc = "start an interactive resize\n\nStart a user-driven, interactive resize of the surface.\n\nThis request must be used in response to some sort of user action\nlike a button press, key press, or touch down event. The passed\nserial is used to determine the type of interactive resize (touch,\npointer, etc).\n\nThe server may ignore resize requests depending on the state of\nthe surface (e.g. fullscreen or maximized).\n\nIf triggered, the client will receive configure events with the\n\"resize\" state enum value and the expected sizes. See the \"resize\"\nenum value for more details about what is required. The client\nmust also acknowledge configure events using \"ack_configure\". After\nthe resize is completed, the client will receive another \"configure\"\nevent without the resize state.\n\nIf triggered, the surface also will lose the focus of the device\n(wl_pointer, wl_touch, etc) used for the resize. It is up to the\ncompositor to visually indicate that the resize is taking place,\nsuch as updating a pointer cursor, during the resize. There is no\nguarantee that the device focus will return when the resize is\ncompleted.\n\nThe edges parameter specifies how the surface should be resized, and\nis one of the values of the resize_edge enum. Values not matching\na variant of the enum will cause the invalid_resize_edge protocol error.\nThe compositor may use this information to update the surface position\nfor example when dragging the top left corner. The compositor may also\nuse this information to adapt its behavior, e.g. choose an appropriate\ncursor image."]
    pub fn resize(&self, seat: &super::wl_seat::WlSeat, serial: u32, edges: ResizeEdge) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Resize { seat: seat.clone(), serial: serial, edges: WEnum::Value(edges) }, None);
    }
    #[doc = "set the maximum size\n\nSet a maximum size for the window.\n\nThe client can specify a maximum size so that the compositor does\nnot try to configure the window beyond this size.\n\nThe width and height arguments are in window geometry coordinates.\nSee xdg_surface.set_window_geometry.\n\nValues set in this way are double-buffered, see wl_surface.commit.\n\nThe compositor can use this information to allow or disallow\ndifferent states like maximize or fullscreen and draw accurate\nanimations.\n\nSimilarly, a tiling window manager may use this information to\nplace and resize client windows in a more effective way.\n\nThe client should not rely on the compositor to obey the maximum\nsize. The compositor may decide to ignore the values set by the\nclient and request a larger size.\n\nIf never set, or a value of zero in the request, means that the\nclient has no expected maximum size in the given dimension.\nAs a result, a client wishing to reset the maximum size\nto an unspecified state can use zero for width and height in the\nrequest.\n\nRequesting a maximum size to be smaller than the minimum size of\na surface is illegal and will result in an invalid_size error.\n\nThe width and height must be greater than or equal to zero. Using\nstrictly negative values for width or height will result in a\ninvalid_size error."]
    pub fn set_max_size(&self, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetMaxSize { width: width, height: height }, None);
    }
    #[doc = "set the minimum size\n\nSet a minimum size for the window.\n\nThe client can specify a minimum size so that the compositor does\nnot try to configure the window below this size.\n\nThe width and height arguments are in window geometry coordinates.\nSee xdg_surface.set_window_geometry.\n\nValues set in this way are double-buffered, see wl_surface.commit.\n\nThe compositor can use this information to allow or disallow\ndifferent states like maximize or fullscreen and draw accurate\nanimations.\n\nSimilarly, a tiling window manager may use this information to\nplace and resize client windows in a more effective way.\n\nThe client should not rely on the compositor to obey the minimum\nsize. The compositor may decide to ignore the values set by the\nclient and request a smaller size.\n\nIf never set, or a value of zero in the request, means that the\nclient has no expected minimum size in the given dimension.\nAs a result, a client wishing to reset the minimum size\nto an unspecified state can use zero for width and height in the\nrequest.\n\nRequesting a minimum size to be larger than the maximum size of\na surface is illegal and will result in an invalid_size error.\n\nThe width and height must be greater than or equal to zero. Using\nstrictly negative values for width and height will result in a\ninvalid_size error."]
    pub fn set_min_size(&self, width: i32, height: i32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetMinSize { width: width, height: height }, None);
    }
    #[doc = "maximize the window\n\nMaximize the surface.\n\nAfter requesting that the surface should be maximized, the compositor\nwill respond by emitting a configure event. Whether this configure\nactually sets the window maximized is subject to compositor policies.\nThe client must then update its content, drawing in the configured\nstate. The client must also acknowledge the configure when committing\nthe new content (see ack_configure).\n\nIt is up to the compositor to decide how and where to maximize the\nsurface, for example which output and what region of the screen should\nbe used.\n\nIf the surface was already maximized, the compositor will still emit\na configure event with the \"maximized\" state.\n\nIf the surface is in a fullscreen state, this request has no direct\neffect. It may alter the state the surface is returned to when\nunmaximized unless overridden by the compositor."]
    pub fn set_maximized(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetMaximized, None);
    }
    #[doc = "unmaximize the window\n\nUnmaximize the surface.\n\nAfter requesting that the surface should be unmaximized, the compositor\nwill respond by emitting a configure event. Whether this actually\nun-maximizes the window is subject to compositor policies.\nIf available and applicable, the compositor will include the window\ngeometry dimensions the window had prior to being maximized in the\nconfigure event. The client must then update its content, drawing it in\nthe configured state. The client must also acknowledge the configure\nwhen committing the new content (see ack_configure).\n\nIt is up to the compositor to position the surface after it was\nunmaximized; usually the position the surface had before maximizing, if\napplicable.\n\nIf the surface was already not maximized, the compositor will still\nemit a configure event without the \"maximized\" state.\n\nIf the surface is in a fullscreen state, this request has no direct\neffect. It may alter the state the surface is returned to when\nunmaximized unless overridden by the compositor."]
    pub fn unset_maximized(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::UnsetMaximized, None);
    }
    #[doc = "set the window as fullscreen on an output\n\nMake the surface fullscreen.\n\nAfter requesting that the surface should be fullscreened, the\ncompositor will respond by emitting a configure event. Whether the\nclient is actually put into a fullscreen state is subject to compositor\npolicies. The client must also acknowledge the configure when\ncommitting the new content (see ack_configure).\n\nThe output passed by the request indicates the client's preference as\nto which display it should be set fullscreen on. If this value is NULL,\nit's up to the compositor to choose which display will be used to map\nthis surface.\n\nIf the surface doesn't cover the whole output, the compositor will\nposition the surface in the center of the output and compensate with\nwith border fill covering the rest of the output. The content of the\nborder fill is undefined, but should be assumed to be in some way that\nattempts to blend into the surrounding area (e.g. solid black).\n\nIf the fullscreened surface is not opaque, the compositor must make\nsure that other screen content not part of the same surface tree (made\nup of subsurfaces, popups or similarly coupled surfaces) are not\nvisible below the fullscreened surface."]
    pub fn set_fullscreen(&self, output: Option<&super::wl_output::WlOutput>) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetFullscreen { output: output.cloned() }, None);
    }
    #[doc = "unset the window as fullscreen\n\nMake the surface no longer fullscreen.\n\nAfter requesting that the surface should be unfullscreened, the\ncompositor will respond by emitting a configure event.\nWhether this actually removes the fullscreen state of the client is\nsubject to compositor policies.\n\nMaking a surface unfullscreen sets states for the surface based on the following:\n* the state(s) it may have had before becoming fullscreen\n* any state(s) decided by the compositor\n* any state(s) requested by the client while the surface was fullscreen\n\nThe compositor may include the previous window geometry dimensions in\nthe configure event, if applicable.\n\nThe client must also acknowledge the configure when committing the new\ncontent (see ack_configure)."]
    pub fn unset_fullscreen(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::UnsetFullscreen, None);
    }
    #[doc = "set the window as minimized\n\nRequest that the compositor minimize your surface. There is no\nway to know if the surface is currently minimized, nor is there\nany way to unset minimization on this surface.\n\nIf you are looking to throttle redrawing when minimized, please\ninstead use the wl_surface.frame event for this, as this will\nalso work with live previews on windows in Alt-Tab, Expose or\nsimilar compositor features."]
    pub fn set_minimized(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::SetMinimized, None);
    }
    }
    }
    #[doc = "short-lived, popup surfaces for menus\n\nA popup surface is a short-lived, temporary surface. It can be used to\nimplement for example menus, popovers, tooltips and other similar user\ninterface concepts.\n\nA popup can be made to take an explicit grab. See xdg_popup.grab for\ndetails.\n\nWhen the popup is dismissed, a popup_done event will be sent out, and at\nthe same time the surface will be unmapped. See the xdg_popup.popup_done\nevent for details.\n\nExplicitly destroying the xdg_popup object will also dismiss the popup and\nunmap the surface. Clients that want to dismiss the popup when another\nsurface of their own is clicked should dismiss the popup using the destroy\nrequest.\n\nA newly created xdg_popup will be stacked on top of all previously created\nxdg_popup surfaces associated with the same xdg_toplevel.\n\nThe parent of an xdg_popup must be mapped (see the xdg_surface\ndescription) before the xdg_popup itself.\n\nThe client must call wl_surface.commit on the corresponding wl_surface\nfor the xdg_popup state to take effect."]
    pub mod xdg_popup {
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
    #[doc = "tried to grab after being mapped"]
        InvalidGrab = 0,
    }
    impl TryFrom<u32> for Error {
        type Error = ();
        fn try_from(value: u32) -> Result<Self, ()> { match value {
            0 => Ok(Self::InvalidGrab),
            _ => Err(()),
        } }
    }
    impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Request<'a> {
    #[doc = "remove xdg_popup interface\n\nThis destroys the popup. Explicitly destroying the xdg_popup\nobject will also dismiss the popup, and unmap the surface.\n\nIf this xdg_popup is not the \"topmost\" popup, the\nxdg_wm_base.not_the_topmost_popup protocol error will be sent."]
    #[doc = "This message destroys the protocol object."]
        Destroy,
    #[doc = "make the popup take an explicit grab\n\nThis request makes the created popup take an explicit grab. An explicit\ngrab will be dismissed when the user dismisses the popup, or when the\nclient destroys the xdg_popup. This can be done by the user clicking\noutside the surface, using the keyboard, or even locking the screen\nthrough closing the lid or a timeout.\n\nIf the compositor denies the grab, the popup will be immediately\ndismissed.\n\nThis request must be used in response to some sort of user action like a\nbutton press, key press, or touch down event. The serial number of the\nevent should be passed as 'serial'.\n\nThe parent of a grabbing popup must either be an xdg_toplevel surface or\nanother xdg_popup with an explicit grab. If the parent is another\nxdg_popup it means that the popups are nested, with this popup now being\nthe topmost popup.\n\nNested popups must be destroyed in the reverse order they were created\nin, e.g. the only popup you are allowed to destroy at all times is the\ntopmost one.\n\nWhen compositors choose to dismiss a popup, they may dismiss every\nnested grabbing popup as well. When a compositor dismisses popups, it\nwill follow the same dismissing order as required from the client.\n\nIf the topmost grabbing popup is destroyed, the grab will be returned to\nthe parent of the popup, if that parent previously had an explicit grab.\n\nIf the parent is a grabbing popup which has already been dismissed, this\npopup will be immediately dismissed. If the parent is a popup that did\nnot take an explicit grab, an error will be raised.\n\nDuring a popup grab, the client owning the grab will receive pointer\nand touch events for all their surfaces as normal (similar to an\n\"owner-events\" grab in X11 parlance), while the top most grabbing popup\nwill always have keyboard focus."]
        Grab {
    #[doc = "the wl_seat of the user event"]
            seat: super::wl_seat::WlSeat,
    #[doc = "the serial of the user event"]
            serial: u32,
        },
    #[doc = "recalculate the popup's location\n\nReposition an already-mapped popup. The popup will be placed given the\ndetails in the passed xdg_positioner object, and a\nxdg_popup.repositioned followed by xdg_popup.configure and\nxdg_surface.configure will be emitted in response. Any parameters set\nby the previous positioner will be discarded.\n\nThe passed token will be sent in the corresponding\nxdg_popup.repositioned event. The new popup position will not take\neffect until the corresponding configure event is acknowledged by the\nclient. See xdg_popup.repositioned for details. The token itself is\nopaque, and has no other special meaning.\n\nIf multiple reposition requests are sent, the compositor may skip all\nbut the last one.\n\nIf the popup is repositioned in response to a configure event for its\nparent, the client should send an xdg_positioner.set_parent_configure\nand possibly an xdg_positioner.set_parent_size request to allow the\ncompositor to properly constrain the popup.\n\nIf the popup is repositioned together with a parent that is being\nresized, but not in response to a configure event, the client should\nsend an xdg_positioner.set_parent_size request."]
    #[doc = "Available since interface version 3."]
        Reposition {
            positioner: super::xdg_positioner::XdgPositioner,
    #[doc = "reposition request token"]
            token: u32,
        },
        #[doc(hidden)]
        __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
    }
    impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
        Self::Destroy => 0,
        Self::Grab { .. } => 1,
        Self::Reposition { .. } => 2,
        Self::__PhantomLifetime { never, .. } => match never {},
    } } }
    pub const REQ_DESTROY_SINCE: u32 = 1;
    pub const REQ_DESTROY_OPCODE: u16 = 0;
    pub const REQ_GRAB_SINCE: u32 = 1;
    pub const REQ_GRAB_OPCODE: u16 = 1;
    pub const REQ_REPOSITION_SINCE: u32 = 3;
    pub const REQ_REPOSITION_OPCODE: u16 = 2;
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum Event {
    #[doc = "configure the popup surface\n\nThis event asks the popup surface to configure itself given the\nconfiguration. The configured state should not be applied immediately.\nSee xdg_surface.configure for details.\n\nThe x and y arguments represent the position the popup was placed at\ngiven the xdg_positioner rule, relative to the upper left corner of the\nwindow geometry of the parent surface.\n\nFor version 2 or older, the configure event for an xdg_popup is only\never sent once for the initial configuration. Starting with version 3,\nit may be sent again if the popup is setup with an xdg_positioner with\nset_reactive requested, or in response to xdg_popup.reposition requests."]
        Configure {
    #[doc = "x position relative to parent surface window geometry"]
            x: i32,
    #[doc = "y position relative to parent surface window geometry"]
            y: i32,
    #[doc = "window geometry width"]
            width: i32,
    #[doc = "window geometry height"]
            height: i32,
        },
    #[doc = "popup interaction is done\n\nThe popup_done event is sent out when a popup is dismissed by the\ncompositor. The client should destroy the xdg_popup object at this\npoint."]
        PopupDone,
    #[doc = "signal the completion of a repositioned request\n\nThe repositioned event is sent as part of a popup configuration\nsequence, together with xdg_popup.configure and lastly\nxdg_surface.configure to notify the completion of a reposition request.\n\nThe repositioned event is to notify about the completion of a\nxdg_popup.reposition request. The token argument is the token passed\nin the xdg_popup.reposition request.\n\nImmediately after this event is emitted, xdg_popup.configure and\nxdg_surface.configure will be sent with the updated size and position,\nas well as a new configure serial.\n\nThe client should optionally update the content of the popup, but must\nacknowledge the new popup configuration for the new position to take\neffect. See xdg_surface.ack_configure for details."]
    #[doc = "Available since interface version 3."]
        Repositioned {
    #[doc = "reposition request token"]
            token: u32,
        },
    }
    impl Event { pub fn opcode(&self) -> u16 { match *self {
        Self::Configure { .. } => 0,
        Self::PopupDone => 1,
        Self::Repositioned { .. } => 2,
    } } }
    pub const EVT_CONFIGURE_SINCE: u32 = 1;
    pub const EVT_CONFIGURE_OPCODE: u16 = 0;
    pub const EVT_POPUP_DONE_SINCE: u32 = 1;
    pub const EVT_POPUP_DONE_OPCODE: u16 = 1;
    pub const EVT_REPOSITIONED_SINCE: u32 = 3;
    pub const EVT_REPOSITIONED_OPCODE: u16 = 2;

    #[derive(Debug, Clone)]
    pub struct XdgPopup {
        id: ObjectId,
        version: u32,
        data: Option<Arc<dyn ObjectData>>,
        backend: WeakBackend,
    }
    impl PartialEq for XdgPopup { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
    impl Eq for XdgPopup {}
    impl PartialEq<Weak<XdgPopup>> for XdgPopup {
        fn eq(&self, other: &Weak<XdgPopup>) -> bool { self.id == other.id() }
    }
    impl std::borrow::Borrow<ObjectId> for XdgPopup { fn borrow(&self) -> &ObjectId { &self.id } }
    impl std::hash::Hash for XdgPopup {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
    }
    impl Proxy for XdgPopup {
        type Request<'request> = Request<'request>;
        type Event = Event;
        #[inline]
        fn interface() -> &'static Interface { &super::XDG_POPUP_INTERFACE }
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
                if let (Some(Argument::Int(x)), Some(Argument::Int(y)), Some(Argument::Int(width)), Some(Argument::Int(height)),) = (arg_iter.next(), arg_iter.next(), arg_iter.next(), arg_iter.next(),) {
                    Ok((me, Event::Configure { x: x, y: y, width: width, height: height }))
                } else {
                    Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                }
            }
            1 => Ok((me, Event::PopupDone)),
            2 => {
                if let (Some(Argument::Uint(token)),) = (arg_iter.next(),) {
                    Ok((me, Event::Repositioned { token: token }))
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
            Request::Grab { seat, serial } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                msg.args.push(Argument::Object(seat.id()));
                msg.args.push(Argument::Uint(serial));
                Ok((msg, child_spec))
            },
            Request::Reposition { positioner, token } => {
                let child_spec = None;
                let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                msg.args.push(Argument::Object(positioner.id()));
                msg.args.push(Argument::Uint(token));
                Ok((msg, child_spec))
            },
            Request::__PhantomLifetime { never, .. } => match never {},
        }
    }
    }
    impl XdgPopup {
    #[doc = "remove xdg_popup interface\n\nThis destroys the popup. Explicitly destroying the xdg_popup\nobject will also dismiss the popup, and unmap the surface.\n\nIf this xdg_popup is not the \"topmost\" popup, the\nxdg_wm_base.not_the_topmost_popup protocol error will be sent."]
    pub fn destroy(&self) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Destroy, None);
    }
    #[doc = "make the popup take an explicit grab\n\nThis request makes the created popup take an explicit grab. An explicit\ngrab will be dismissed when the user dismisses the popup, or when the\nclient destroys the xdg_popup. This can be done by the user clicking\noutside the surface, using the keyboard, or even locking the screen\nthrough closing the lid or a timeout.\n\nIf the compositor denies the grab, the popup will be immediately\ndismissed.\n\nThis request must be used in response to some sort of user action like a\nbutton press, key press, or touch down event. The serial number of the\nevent should be passed as 'serial'.\n\nThe parent of a grabbing popup must either be an xdg_toplevel surface or\nanother xdg_popup with an explicit grab. If the parent is another\nxdg_popup it means that the popups are nested, with this popup now being\nthe topmost popup.\n\nNested popups must be destroyed in the reverse order they were created\nin, e.g. the only popup you are allowed to destroy at all times is the\ntopmost one.\n\nWhen compositors choose to dismiss a popup, they may dismiss every\nnested grabbing popup as well. When a compositor dismisses popups, it\nwill follow the same dismissing order as required from the client.\n\nIf the topmost grabbing popup is destroyed, the grab will be returned to\nthe parent of the popup, if that parent previously had an explicit grab.\n\nIf the parent is a grabbing popup which has already been dismissed, this\npopup will be immediately dismissed. If the parent is a popup that did\nnot take an explicit grab, an error will be raised.\n\nDuring a popup grab, the client owning the grab will receive pointer\nand touch events for all their surfaces as normal (similar to an\n\"owner-events\" grab in X11 parlance), while the top most grabbing popup\nwill always have keyboard focus."]
    pub fn grab(&self, seat: &super::wl_seat::WlSeat, serial: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Grab { seat: seat.clone(), serial: serial }, None);
    }
    #[doc = "recalculate the popup's location\n\nReposition an already-mapped popup. The popup will be placed given the\ndetails in the passed xdg_positioner object, and a\nxdg_popup.repositioned followed by xdg_popup.configure and\nxdg_surface.configure will be emitted in response. Any parameters set\nby the previous positioner will be discarded.\n\nThe passed token will be sent in the corresponding\nxdg_popup.repositioned event. The new popup position will not take\neffect until the corresponding configure event is acknowledged by the\nclient. See xdg_popup.repositioned for details. The token itself is\nopaque, and has no other special meaning.\n\nIf multiple reposition requests are sent, the compositor may skip all\nbut the last one.\n\nIf the popup is repositioned in response to a configure event for its\nparent, the client should send an xdg_positioner.set_parent_configure\nand possibly an xdg_positioner.set_parent_size request to allow the\ncompositor to properly constrain the popup.\n\nIf the popup is repositioned together with a parent that is being\nresized, but not in response to a configure event, the client should\nsend an xdg_positioner.set_parent_size request."]
    pub fn reposition(&self, positioner: &super::xdg_positioner::XdgPositioner, token: u32) -> () {
        let Some(backend) = self.backend.upgrade() else { return; };
        let conn = Connection::from_backend(backend);
        let _ = conn.send_request(self, Request::Reposition { positioner: positioner.clone(), token: token }, None);
    }
    }
    }

    }
}

#[cfg(feature = "staging")]
pub mod toplevel_icon {
    pub mod v1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2023-2024 Matthias Klumpp
        // Copyright ©      2024 David Edmundson
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
            use crate::xdg::shell::{client::*};
            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                use crate::xdg::shell::{client::__interfaces::*};
                // Immutable libwayland metadata, shared by every connection.
        struct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);
        unsafe impl Sync for InterfacePointers {}
        struct Messages(&'static [wayland_backend::protocol::wl_message]);
        unsafe impl Sync for Messages {}
        pub static XDG_TOPLEVEL_ICON_MANAGER_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "xdg_toplevel_icon_manager_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "create_icon", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&XDG_TOPLEVEL_ICON_V1_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_icon", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::Yes),],
                    child_interface: None, arg_interfaces: &[&XDG_TOPLEVEL_INTERFACE,&XDG_TOPLEVEL_ICON_V1_INTERFACE,],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "icon_size", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&xdg_toplevel_icon_manager_v1_interface),
        };
        static xdg_toplevel_icon_manager_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_toplevel_icon_manager_v1_requests_create_icon_types: InterfacePointers = InterfacePointers(&[&xdg_toplevel_icon_v1_interface,]);
        static xdg_toplevel_icon_manager_v1_requests_set_icon_types: InterfacePointers = InterfacePointers(&[&xdg_toplevel_interface,&xdg_toplevel_icon_v1_interface,]);
        static xdg_toplevel_icon_manager_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_icon_manager_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"create_icon".as_ptr(), signature: c"n".as_ptr(), types: xdg_toplevel_icon_manager_v1_requests_create_icon_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_icon".as_ptr(), signature: c"o?o".as_ptr(), types: xdg_toplevel_icon_manager_v1_requests_set_icon_types.0.as_ptr() },
        ]);
        static xdg_toplevel_icon_manager_v1_events_icon_size_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static xdg_toplevel_icon_manager_v1_events_done_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_toplevel_icon_manager_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"icon_size".as_ptr(), signature: c"i".as_ptr(), types: xdg_toplevel_icon_manager_v1_events_icon_size_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_icon_manager_v1_events_done_types.0.as_ptr() },
        ]);
        pub static xdg_toplevel_icon_manager_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"xdg_toplevel_icon_manager_v1".as_ptr(), version: 1, request_count: 3, requests: xdg_toplevel_icon_manager_v1_requests.0.as_ptr(), event_count: 2, events: xdg_toplevel_icon_manager_v1_events.0.as_ptr(),
        };
        pub static XDG_TOPLEVEL_ICON_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "xdg_toplevel_icon_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_name", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "add_buffer", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Int,],
                    child_interface: None, arg_interfaces: &[&WL_BUFFER_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&xdg_toplevel_icon_v1_interface),
        };
        static xdg_toplevel_icon_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_toplevel_icon_v1_requests_set_name_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static xdg_toplevel_icon_v1_requests_add_buffer_types: InterfacePointers = InterfacePointers(&[&wl_buffer_interface,core::ptr::null(),]);
        static xdg_toplevel_icon_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_toplevel_icon_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_name".as_ptr(), signature: c"s".as_ptr(), types: xdg_toplevel_icon_v1_requests_set_name_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"add_buffer".as_ptr(), signature: c"oi".as_ptr(), types: xdg_toplevel_icon_v1_requests_add_buffer_types.0.as_ptr() },
        ]);
        pub static xdg_toplevel_icon_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"xdg_toplevel_icon_v1".as_ptr(), version: 1, request_count: 3, requests: xdg_toplevel_icon_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };

            }
            use self::__interfaces::*;
            #[doc = "interface to manage toplevel icons\n\nThis interface allows clients to create toplevel window icons and set\nthem on toplevel windows to be displayed to the user."]
        pub mod xdg_toplevel_icon_manager_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the toplevel icon manager\n\nDestroy the toplevel icon manager.\nThis does not destroy objects created with the manager."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "create a new icon instance\n\nCreates a new icon object. This icon can then be attached to a\nxdg_toplevel via the 'set_icon' request."]
            CreateIcon {
            },
        #[doc = "set an icon on a toplevel window\n\nThis request assigns the icon 'icon' to 'toplevel', or clears the\ntoplevel icon if 'icon' was null.\nThis state is double-buffered and is applied on the next\nwl_surface.commit of the toplevel.\n\nAfter making this call, the xdg_toplevel_icon_v1 provided as 'icon'\ncan be destroyed by the client without 'toplevel' losing its icon.\nThe xdg_toplevel_icon_v1 is immutable from this point, and any\nfuture attempts to change it must raise the\n'xdg_toplevel_icon_v1.immutable' protocol error.\n\nThe compositor must set the toplevel icon from either the pixel data\nthe icon provides, or by loading a stock icon using the icon name.\nSee the description of 'xdg_toplevel_icon_v1' for details.\n\nIf 'icon' is set to null, the icon of the respective toplevel is reset\nto its default icon (usually the icon of the application, derived from\nits desktop-entry file, or a placeholder icon).\nIf this request is passed an icon with no pixel buffers or icon name\nassigned, the icon must be reset just like if 'icon' was null."]
            SetIcon {
        #[doc = "the toplevel to act on"]
                toplevel: super::xdg_toplevel::XdgToplevel,
                icon: Option<super::xdg_toplevel_icon_v1::XdgToplevelIconV1>,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::CreateIcon { .. } => 1,
            Self::SetIcon { .. } => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_CREATE_ICON_SINCE: u32 = 1;
        pub const REQ_CREATE_ICON_OPCODE: u16 = 1;
        pub const REQ_SET_ICON_SINCE: u32 = 1;
        pub const REQ_SET_ICON_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "describes a supported & preferred icon size\n\nThis event indicates an icon size the compositor prefers to be\navailable if the client has scalable icons and can render to any size.\n\nWhen the 'xdg_toplevel_icon_manager_v1' object is created, the\ncompositor may send one or more 'icon_size' events to describe the list\nof preferred icon sizes. If the compositor has no size preference, it\nmay not send any 'icon_size' event, and it is up to the client to\ndecide a suitable icon size.\n\nA sequence of 'icon_size' events must be finished with a 'done' event.\nIf the compositor has no size preferences, it must still send the\n'done' event, without any preceding 'icon_size' events."]
            IconSize {
        #[doc = "the edge size of the square icon in surface-local coordinates, e.g. 64"]
                size: i32,
            },
        #[doc = "all information has been sent\n\nThis event is sent after all 'icon_size' events have been sent."]
            Done,
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::IconSize { .. } => 0,
            Self::Done => 1,
        } } }
        pub const EVT_ICON_SIZE_SINCE: u32 = 1;
        pub const EVT_ICON_SIZE_OPCODE: u16 = 0;
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 1;

        #[derive(Debug, Clone)]
        pub struct XdgToplevelIconManagerV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for XdgToplevelIconManagerV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for XdgToplevelIconManagerV1 {}
        impl PartialEq<Weak<XdgToplevelIconManagerV1>> for XdgToplevelIconManagerV1 {
            fn eq(&self, other: &Weak<XdgToplevelIconManagerV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for XdgToplevelIconManagerV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for XdgToplevelIconManagerV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for XdgToplevelIconManagerV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::XDG_TOPLEVEL_ICON_MANAGER_V1_INTERFACE }
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
                    if let (Some(Argument::Int(size)),) = (arg_iter.next(),) {
                        Ok((me, Event::IconSize { size: size }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                1 => Ok((me, Event::Done)),
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::CreateIcon {  } => {
                    let child_spec = Some((super::xdg_toplevel_icon_v1::XdgToplevelIconV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    Ok((msg, child_spec))
                },
                Request::SetIcon { toplevel, icon } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    msg.args.push(Argument::Object(toplevel.id()));
                    msg.args.push(Argument::Object(icon.as_ref().map_or_else(ObjectId::null, Proxy::id)));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl XdgToplevelIconManagerV1 {
        #[doc = "destroy the toplevel icon manager\n\nDestroy the toplevel icon manager.\nThis does not destroy objects created with the manager."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "create a new icon instance\n\nCreates a new icon object. This icon can then be attached to a\nxdg_toplevel via the 'set_icon' request."]
        pub fn create_icon<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_toplevel_icon_v1::XdgToplevelIconV1, U> + 'static>(&self, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_toplevel_icon_v1::XdgToplevelIconV1 {
            self.send_constructor::<super::xdg_toplevel_icon_v1::XdgToplevelIconV1>(Request::CreateIcon {  }, qh.make_data::<super::xdg_toplevel_icon_v1::XdgToplevelIconV1, U>(udata)).unwrap_or_else(|_| super::xdg_toplevel_icon_v1::XdgToplevelIconV1::inert(self.backend.clone()))
        }
        #[doc = "set an icon on a toplevel window\n\nThis request assigns the icon 'icon' to 'toplevel', or clears the\ntoplevel icon if 'icon' was null.\nThis state is double-buffered and is applied on the next\nwl_surface.commit of the toplevel.\n\nAfter making this call, the xdg_toplevel_icon_v1 provided as 'icon'\ncan be destroyed by the client without 'toplevel' losing its icon.\nThe xdg_toplevel_icon_v1 is immutable from this point, and any\nfuture attempts to change it must raise the\n'xdg_toplevel_icon_v1.immutable' protocol error.\n\nThe compositor must set the toplevel icon from either the pixel data\nthe icon provides, or by loading a stock icon using the icon name.\nSee the description of 'xdg_toplevel_icon_v1' for details.\n\nIf 'icon' is set to null, the icon of the respective toplevel is reset\nto its default icon (usually the icon of the application, derived from\nits desktop-entry file, or a placeholder icon).\nIf this request is passed an icon with no pixel buffers or icon name\nassigned, the icon must be reset just like if 'icon' was null."]
        pub fn set_icon(&self, toplevel: &super::xdg_toplevel::XdgToplevel, icon: Option<&super::xdg_toplevel_icon_v1::XdgToplevelIconV1>) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetIcon { toplevel: toplevel.clone(), icon: icon.cloned() }, None);
        }
        }
        }
        #[doc = "a toplevel window icon\n\nThis interface defines a toplevel icon.\nAn icon can have a name, and multiple buffers.\nIn order to be applied, the icon must have either a name, or at least\none buffer assigned. Applying an empty icon (with no buffer or name) to\na toplevel should reset its icon to the default icon.\n\nIt is up to compositor policy whether to prefer using a buffer or loading\nan icon via its name. See 'set_name' and 'add_buffer' for details."]
        pub mod xdg_toplevel_icon_v1 {
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
        #[doc = "the provided buffer does not satisfy requirements"]
            InvalidBuffer = 1,
        #[doc = "the icon has already been assigned to a toplevel and must not be changed"]
            Immutable = 2,
        #[doc = "the provided buffer has been destroyed before the toplevel icon"]
            NoBuffer = 3,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                1 => Ok(Self::InvalidBuffer),
                2 => Ok(Self::Immutable),
                3 => Ok(Self::NoBuffer),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the icon object\n\nDestroys the 'xdg_toplevel_icon_v1' object.\nThe icon must still remain set on every toplevel it was assigned to,\nuntil the toplevel icon is reset explicitly."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "set an icon name\n\nThis request assigns an icon name to this icon.\nAny previously set name is overridden.\n\nThe compositor must resolve 'icon_name' according to the lookup rules\ndescribed in the XDG icon theme specification[1] using the\nenvironment's current icon theme.\n\nIf the compositor does not support icon names or cannot resolve\n'icon_name' according to the XDG icon theme specification it must\nfall back to using pixel buffer data instead.\n\nIf this request is made after the icon has been assigned to a toplevel\nvia 'set_icon', a 'immutable' error must be raised.\n\n[1]: https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html"]
            SetName {
                icon_name: String,
            },
        #[doc = "add icon data from a pixel buffer\n\nThis request adds pixel data supplied as wl_buffer to the icon.\n\nThe client should add pixel data for all icon sizes and scales that\nit can provide, or which are explicitly requested by the compositor\nvia 'icon_size' events on xdg_toplevel_icon_manager_v1.\n\nThe wl_buffer supplying pixel data as 'buffer' must be backed by wl_shm\nand must be a square (width and height being equal).\nIf any of these buffer requirements are not fulfilled, a 'invalid_buffer'\nerror must be raised.\n\nIf this icon instance already has a buffer of the same size and scale\nfrom a previous 'add_buffer' request, data from the last request\noverrides the preexisting pixel data.\n\nThe wl_buffer must be kept alive for as long as the xdg_toplevel_icon\nit is associated with is not destroyed, otherwise a 'no_buffer' error\nis raised. The buffer contents must not be modified after it was\nassigned to the icon. As a result, the region of the wl_shm_pool's\nbacking storage used for the wl_buffer must not be modified after this\nrequest is sent. The wl_buffer.release event is unused.\n\nIf this request is made after the icon has been assigned to a toplevel\nvia 'set_icon', a 'immutable' error must be raised."]
            AddBuffer {
                buffer: super::wl_buffer::WlBuffer,
        #[doc = "the scaling factor of the icon, e.g. 1"]
                scale: i32,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::SetName { .. } => 1,
            Self::AddBuffer { .. } => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_SET_NAME_SINCE: u32 = 1;
        pub const REQ_SET_NAME_OPCODE: u16 = 1;
        pub const REQ_ADD_BUFFER_SINCE: u32 = 1;
        pub const REQ_ADD_BUFFER_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct XdgToplevelIconV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for XdgToplevelIconV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for XdgToplevelIconV1 {}
        impl PartialEq<Weak<XdgToplevelIconV1>> for XdgToplevelIconV1 {
            fn eq(&self, other: &Weak<XdgToplevelIconV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for XdgToplevelIconV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for XdgToplevelIconV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for XdgToplevelIconV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::XDG_TOPLEVEL_ICON_V1_INTERFACE }
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
                Request::SetName { icon_name } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(icon_name).unwrap()))));
                    Ok((msg, child_spec))
                },
                Request::AddBuffer { buffer, scale } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    msg.args.push(Argument::Object(buffer.id()));
                    msg.args.push(Argument::Int(scale));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl XdgToplevelIconV1 {
        #[doc = "destroy the icon object\n\nDestroys the 'xdg_toplevel_icon_v1' object.\nThe icon must still remain set on every toplevel it was assigned to,\nuntil the toplevel icon is reset explicitly."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "set an icon name\n\nThis request assigns an icon name to this icon.\nAny previously set name is overridden.\n\nThe compositor must resolve 'icon_name' according to the lookup rules\ndescribed in the XDG icon theme specification[1] using the\nenvironment's current icon theme.\n\nIf the compositor does not support icon names or cannot resolve\n'icon_name' according to the XDG icon theme specification it must\nfall back to using pixel buffer data instead.\n\nIf this request is made after the icon has been assigned to a toplevel\nvia 'set_icon', a 'immutable' error must be raised.\n\n[1]: https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html"]
        pub fn set_name(&self, icon_name: String) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetName { icon_name: icon_name }, None);
        }
        #[doc = "add icon data from a pixel buffer\n\nThis request adds pixel data supplied as wl_buffer to the icon.\n\nThe client should add pixel data for all icon sizes and scales that\nit can provide, or which are explicitly requested by the compositor\nvia 'icon_size' events on xdg_toplevel_icon_manager_v1.\n\nThe wl_buffer supplying pixel data as 'buffer' must be backed by wl_shm\nand must be a square (width and height being equal).\nIf any of these buffer requirements are not fulfilled, a 'invalid_buffer'\nerror must be raised.\n\nIf this icon instance already has a buffer of the same size and scale\nfrom a previous 'add_buffer' request, data from the last request\noverrides the preexisting pixel data.\n\nThe wl_buffer must be kept alive for as long as the xdg_toplevel_icon\nit is associated with is not destroyed, otherwise a 'no_buffer' error\nis raised. The buffer contents must not be modified after it was\nassigned to the icon. As a result, the region of the wl_shm_pool's\nbacking storage used for the wl_buffer must not be modified after this\nrequest is sent. The wl_buffer.release event is unused.\n\nIf this request is made after the icon has been assigned to a toplevel\nvia 'set_icon', a 'immutable' error must be raised."]
        pub fn add_buffer(&self, buffer: &super::wl_buffer::WlBuffer, scale: i32) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::AddBuffer { buffer: buffer.clone(), scale: scale }, None);
        }
        }
        }

        }
    }
}

#[cfg(feature = "staging")]
pub mod activation {
    pub mod v1 {
        pub mod client {
            #![allow(non_upper_case_globals)]
        // Copyright © 2020 Aleix Pol Gonzalez <aleixpol@kde.org>
        // Copyright © 2020 Carlos Garnacho <carlosg@gnome.org>
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
        pub static XDG_ACTIVATION_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "xdg_activation_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "get_activation_token", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::NewId,],
                    child_interface: Some(&XDG_ACTIVATION_TOKEN_V1_INTERFACE), arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "activate", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
            ],
            events: &[
            ],
            c_ptr: Some(&xdg_activation_v1_interface),
        };
        static xdg_activation_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_activation_v1_requests_get_activation_token_types: InterfacePointers = InterfacePointers(&[&xdg_activation_token_v1_interface,]);
        static xdg_activation_v1_requests_activate_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&wl_surface_interface,]);
        static xdg_activation_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_activation_v1_requests_destroy_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"get_activation_token".as_ptr(), signature: c"n".as_ptr(), types: xdg_activation_v1_requests_get_activation_token_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"activate".as_ptr(), signature: c"so".as_ptr(), types: xdg_activation_v1_requests_activate_types.0.as_ptr() },
        ]);
        pub static xdg_activation_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"xdg_activation_v1".as_ptr(), version: 1, request_count: 3, requests: xdg_activation_v1_requests.0.as_ptr(), event_count: 0, events: core::ptr::null(),
        };
        pub static XDG_ACTIVATION_TOKEN_V1_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {
            name: "xdg_activation_token_v1", version: 1,
            requests: &[
                wayland_backend::protocol::MessageDesc { name: "set_serial", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Uint,wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SEAT_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "set_app_id", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "set_surface", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Object(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[&WL_SURFACE_INTERFACE,],
                },
                wayland_backend::protocol::MessageDesc { name: "commit", since: 1, is_destructor: false,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
                wayland_backend::protocol::MessageDesc { name: "destroy", since: 1, is_destructor: true,
                    signature: &[],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            events: &[
                wayland_backend::protocol::MessageDesc { name: "done", since: 1, is_destructor: false,
                    signature: &[wayland_backend::protocol::ArgumentType::Str(wayland_backend::protocol::AllowNull::No),],
                    child_interface: None, arg_interfaces: &[],
                },
            ],
            c_ptr: Some(&xdg_activation_token_v1_interface),
        };
        static xdg_activation_token_v1_requests_set_serial_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),&wl_seat_interface,]);
        static xdg_activation_token_v1_requests_set_app_id_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static xdg_activation_token_v1_requests_set_surface_types: InterfacePointers = InterfacePointers(&[&wl_surface_interface,]);
        static xdg_activation_token_v1_requests_commit_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_activation_token_v1_requests_destroy_types: InterfacePointers = InterfacePointers(&[]);
        static xdg_activation_token_v1_requests: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"set_serial".as_ptr(), signature: c"uo".as_ptr(), types: xdg_activation_token_v1_requests_set_serial_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_app_id".as_ptr(), signature: c"s".as_ptr(), types: xdg_activation_token_v1_requests_set_app_id_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"set_surface".as_ptr(), signature: c"o".as_ptr(), types: xdg_activation_token_v1_requests_set_surface_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"commit".as_ptr(), signature: c"".as_ptr(), types: xdg_activation_token_v1_requests_commit_types.0.as_ptr() },
            wayland_backend::protocol::wl_message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: xdg_activation_token_v1_requests_destroy_types.0.as_ptr() },
        ]);
        static xdg_activation_token_v1_events_done_types: InterfacePointers = InterfacePointers(&[core::ptr::null(),]);
        static xdg_activation_token_v1_events: Messages = Messages(&[
            wayland_backend::protocol::wl_message { name: c"done".as_ptr(), signature: c"s".as_ptr(), types: xdg_activation_token_v1_events_done_types.0.as_ptr() },
        ]);
        pub static xdg_activation_token_v1_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {
            name: c"xdg_activation_token_v1".as_ptr(), version: 1, request_count: 5, requests: xdg_activation_token_v1_requests.0.as_ptr(), event_count: 1, events: xdg_activation_token_v1_events.0.as_ptr(),
        };

            }
            use self::__interfaces::*;
            #[doc = "interface for activating surfaces\n\nA global interface used for informing the compositor about applications\nbeing activated or started, or for applications to request to be\nactivated."]
        pub mod xdg_activation_v1 {
        use super::wayland_client;
        use std::sync::Arc;
        use std::os::fd::OwnedFd;
        use wayland_client::{Proxy, Connection, DispatchError, Weak};
        use wayland_client::backend::{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{Message, Interface, same_interface}};
        use wayland_client::backend::protocol::Argument;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "destroy the xdg_activation object\n\nNotify the compositor that the xdg_activation object will no longer be\nused.\n\nThe child objects created via this interface are unaffected and should\nbe destroyed separately."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
        #[doc = "requests a token\n\nCreates an xdg_activation_token_v1 object that will provide\nthe initiating client with a unique token for this activation. This\ntoken should be offered to the clients to be activated."]
            GetActivationToken {
            },
        #[doc = "notify new interaction being available\n\nRequests surface activation. It's up to the compositor to display\nthis information as desired, for example by placing the surface above\nthe rest.\n\nThe compositor may know who requested this by checking the activation\ntoken and might decide not to follow through with the activation if it's\nconsidered unwanted.\n\nCompositors can ignore unknown activation tokens when an invalid\ntoken is passed."]
            Activate {
        #[doc = "the activation token of the initiating client"]
                token: String,
        #[doc = "the wl_surface to activate"]
                surface: super::wl_surface::WlSurface,
            },
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::Destroy => 0,
            Self::GetActivationToken { .. } => 1,
            Self::Activate { .. } => 2,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 0;
        pub const REQ_GET_ACTIVATION_TOKEN_SINCE: u32 = 1;
        pub const REQ_GET_ACTIVATION_TOKEN_OPCODE: u16 = 1;
        pub const REQ_ACTIVATE_SINCE: u32 = 1;
        pub const REQ_ACTIVATE_OPCODE: u16 = 2;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
        } } }

        #[derive(Debug, Clone)]
        pub struct XdgActivationV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for XdgActivationV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for XdgActivationV1 {}
        impl PartialEq<Weak<XdgActivationV1>> for XdgActivationV1 {
            fn eq(&self, other: &Weak<XdgActivationV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for XdgActivationV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for XdgActivationV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for XdgActivationV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::XDG_ACTIVATION_V1_INTERFACE }
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
                Request::GetActivationToken {  } => {
                    let child_spec = Some((super::xdg_activation_token_v1::XdgActivationTokenV1::interface(), conn.object_info(self.id())?.version));
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::NewId(ObjectId::null()));
                    Ok((msg, child_spec))
                },
                Request::Activate { token, surface } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(token).unwrap()))));
                    msg.args.push(Argument::Object(surface.id()));
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl XdgActivationV1 {
        #[doc = "destroy the xdg_activation object\n\nNotify the compositor that the xdg_activation object will no longer be\nused.\n\nThe child objects created via this interface are unaffected and should\nbe destroyed separately."]
        pub fn destroy(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Destroy, None);
        }
        #[doc = "requests a token\n\nCreates an xdg_activation_token_v1 object that will provide\nthe initiating client with a unique token for this activation. This\ntoken should be offered to the clients to be activated."]
        pub fn get_activation_token<U: Send + Sync + 'static, D: wayland_client::Dispatch<super::xdg_activation_token_v1::XdgActivationTokenV1, U> + 'static>(&self, qh: &wayland_client::QueueHandle<D>, udata: U) -> super::xdg_activation_token_v1::XdgActivationTokenV1 {
            self.send_constructor::<super::xdg_activation_token_v1::XdgActivationTokenV1>(Request::GetActivationToken {  }, qh.make_data::<super::xdg_activation_token_v1::XdgActivationTokenV1, U>(udata)).unwrap_or_else(|_| super::xdg_activation_token_v1::XdgActivationTokenV1::inert(self.backend.clone()))
        }
        #[doc = "notify new interaction being available\n\nRequests surface activation. It's up to the compositor to display\nthis information as desired, for example by placing the surface above\nthe rest.\n\nThe compositor may know who requested this by checking the activation\ntoken and might decide not to follow through with the activation if it's\nconsidered unwanted.\n\nCompositors can ignore unknown activation tokens when an invalid\ntoken is passed."]
        pub fn activate(&self, token: String, surface: &super::wl_surface::WlSurface) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Activate { token: token, surface: surface.clone() }, None);
        }
        }
        }
        #[doc = "an exported activation handle\n\nAn object for setting up a token and receiving a token handle that can\nbe passed as an activation token to another client.\n\nThe object is created using the xdg_activation_v1.get_activation_token\nrequest. This object should then be populated with the app_id, surface\nand serial information and committed. The compositor shall then issue a\ndone event with the token. In case the request's parameters are invalid,\nthe compositor will provide an invalid token."]
        pub mod xdg_activation_token_v1 {
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
        #[doc = "The token has already been used previously"]
            AlreadyUsed = 0,
        }
        impl TryFrom<u32> for Error {
            type Error = ();
            fn try_from(value: u32) -> Result<Self, ()> { match value {
                0 => Ok(Self::AlreadyUsed),
                _ => Err(()),
            } }
        }
        impl From<Error> for u32 { fn from(value: Error) -> u32 { value as u32 } }
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Request<'a> {
        #[doc = "specifies the seat and serial of the activating event\n\nProvides information about the seat and serial event that requested the\ntoken.\n\nThe serial can come from an input or focus event. For instance, if a\nclick triggers the launch of a third-party client, the launcher client\nshould send a set_serial request with the serial and seat from the\nwl_pointer.button event.\n\nSome compositors might refuse to activate toplevels when the token\ndoesn't have a valid and recent enough event serial.\n\nMust be sent before commit. This information is optional."]
            SetSerial {
        #[doc = "the serial of the event that triggered the activation"]
                serial: u32,
        #[doc = "the wl_seat of the event"]
                seat: super::wl_seat::WlSeat,
            },
        #[doc = "specifies the application being activated\n\nThe requesting client can specify an app_id to associate the token\nbeing created with it.\n\nMust be sent before commit. This information is optional."]
            SetAppId {
        #[doc = "the application id of the client being activated."]
                app_id: String,
            },
        #[doc = "specifies the surface requesting activation\n\nThis request sets the surface requesting the activation. Note, this is\ndifferent from the surface that will be activated.\n\nSome compositors might refuse to activate toplevels when the token\ndoesn't have a requesting surface.\n\nMust be sent before commit. This information is optional."]
            SetSurface {
        #[doc = "the requesting surface"]
                surface: super::wl_surface::WlSurface,
            },
        #[doc = "issues the token request\n\nRequests an activation token based on the different parameters that\nhave been offered through set_serial, set_surface and set_app_id."]
            Commit,
        #[doc = "destroy the xdg_activation_token_v1 object\n\nNotify the compositor that the xdg_activation_token_v1 object will no\nlonger be used. The received token stays valid."]
        #[doc = "This message destroys the protocol object."]
            Destroy,
            #[doc(hidden)]
            __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },
        }
        impl<'a> Request<'a> { pub fn opcode(&self) -> u16 { match *self {
            Self::SetSerial { .. } => 0,
            Self::SetAppId { .. } => 1,
            Self::SetSurface { .. } => 2,
            Self::Commit => 3,
            Self::Destroy => 4,
            Self::__PhantomLifetime { never, .. } => match never {},
        } } }
        pub const REQ_SET_SERIAL_SINCE: u32 = 1;
        pub const REQ_SET_SERIAL_OPCODE: u16 = 0;
        pub const REQ_SET_APP_ID_SINCE: u32 = 1;
        pub const REQ_SET_APP_ID_OPCODE: u16 = 1;
        pub const REQ_SET_SURFACE_SINCE: u32 = 1;
        pub const REQ_SET_SURFACE_OPCODE: u16 = 2;
        pub const REQ_COMMIT_SINCE: u32 = 1;
        pub const REQ_COMMIT_OPCODE: u16 = 3;
        pub const REQ_DESTROY_SINCE: u32 = 1;
        pub const REQ_DESTROY_OPCODE: u16 = 4;
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Event {
        #[doc = "the exported activation token\n\nThe 'done' event contains the unique token of this activation request\nand notifies that the provider is done."]
            Done {
        #[doc = "the exported activation token"]
                token: String,
            },
        }
        impl Event { pub fn opcode(&self) -> u16 { match *self {
            Self::Done { .. } => 0,
        } } }
        pub const EVT_DONE_SINCE: u32 = 1;
        pub const EVT_DONE_OPCODE: u16 = 0;

        #[derive(Debug, Clone)]
        pub struct XdgActivationTokenV1 {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for XdgActivationTokenV1 { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
        impl Eq for XdgActivationTokenV1 {}
        impl PartialEq<Weak<XdgActivationTokenV1>> for XdgActivationTokenV1 {
            fn eq(&self, other: &Weak<XdgActivationTokenV1>) -> bool { self.id == other.id() }
        }
        impl std::borrow::Borrow<ObjectId> for XdgActivationTokenV1 { fn borrow(&self) -> &ObjectId { &self.id } }
        impl std::hash::Hash for XdgActivationTokenV1 {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
        }
        impl Proxy for XdgActivationTokenV1 {
            type Request<'request> = Request<'request>;
            type Event = Event;
            #[inline]
            fn interface() -> &'static Interface { &super::XDG_ACTIVATION_TOKEN_V1_INTERFACE }
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
                    if let (Some(Argument::Str(token)),) = (arg_iter.next(),) {
                        Ok((me, Event::Done { token: String::from_utf8_lossy(token.as_ref().unwrap().as_bytes()).into_owned() }))
                    } else {
                        Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode })
                    }
                }
                _ => Err(DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }),
            }
        }
        fn write_request<'a>(&self, _conn: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {
            match request {
                Request::SetSerial { serial, seat } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 0, args: Default::default() };
                    msg.args.push(Argument::Uint(serial));
                    msg.args.push(Argument::Object(seat.id()));
                    Ok((msg, child_spec))
                },
                Request::SetAppId { app_id } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 1, args: Default::default() };
                    msg.args.push(Argument::Str(Some(Box::new(std::ffi::CString::new(app_id).unwrap()))));
                    Ok((msg, child_spec))
                },
                Request::SetSurface { surface } => {
                    let child_spec = None;
                    let mut msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 2, args: Default::default() };
                    msg.args.push(Argument::Object(surface.id()));
                    Ok((msg, child_spec))
                },
                Request::Commit => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 3, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::Destroy => {
                    let child_spec = None;
                    let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message { sender_id: self.id.clone(), opcode: 4, args: Default::default() };
                    Ok((msg, child_spec))
                },
                Request::__PhantomLifetime { never, .. } => match never {},
            }
        }
        }
        impl XdgActivationTokenV1 {
        #[doc = "specifies the seat and serial of the activating event\n\nProvides information about the seat and serial event that requested the\ntoken.\n\nThe serial can come from an input or focus event. For instance, if a\nclick triggers the launch of a third-party client, the launcher client\nshould send a set_serial request with the serial and seat from the\nwl_pointer.button event.\n\nSome compositors might refuse to activate toplevels when the token\ndoesn't have a valid and recent enough event serial.\n\nMust be sent before commit. This information is optional."]
        pub fn set_serial(&self, serial: u32, seat: &super::wl_seat::WlSeat) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetSerial { serial: serial, seat: seat.clone() }, None);
        }
        #[doc = "specifies the application being activated\n\nThe requesting client can specify an app_id to associate the token\nbeing created with it.\n\nMust be sent before commit. This information is optional."]
        pub fn set_app_id(&self, app_id: String) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetAppId { app_id: app_id }, None);
        }
        #[doc = "specifies the surface requesting activation\n\nThis request sets the surface requesting the activation. Note, this is\ndifferent from the surface that will be activated.\n\nSome compositors might refuse to activate toplevels when the token\ndoesn't have a requesting surface.\n\nMust be sent before commit. This information is optional."]
        pub fn set_surface(&self, surface: &super::wl_surface::WlSurface) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::SetSurface { surface: surface.clone() }, None);
        }
        #[doc = "issues the token request\n\nRequests an activation token based on the different parameters that\nhave been offered through set_serial, set_surface and set_app_id."]
        pub fn commit(&self) -> () {
            let Some(backend) = self.backend.upgrade() else { return; };
            let conn = Connection::from_backend(backend);
            let _ = conn.send_request(self, Request::Commit, None);
        }
        #[doc = "destroy the xdg_activation_token_v1 object\n\nNotify the compositor that the xdg_activation_token_v1 object will no\nlonger be used. The received token stays valid."]
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
