//! The two optional pointer protocols absent from the repository's trimmed Wayland bindings.
//! Uses the same Proxy/ObjectData dispatch and libwayland metadata as those bindings, so these
//! objects are read by the shell's event queue, never a second C listener or event loop.
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::Arc;
use wayland_client::backend::protocol::{
    same_interface, wl_interface, wl_message, AllowNull, Argument, ArgumentType as A, Interface,
    Message, MessageDesc,
};
use wayland_client::backend::{InvalidId, ObjectData, ObjectId, WeakBackend};
use wayland_client::protocol::{
    __interfaces::*, wl_pointer::WlPointer, wl_region::WlRegion, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, DispatchError, Proxy, QueueHandle};

#[derive(Debug)]
pub(super) struct Request<'a> {
    opcode: u16,
    args: Vec<Argument<ObjectId, BorrowedFd<'a>>>,
    child: Option<(&'static Interface, u32)>,
}
#[derive(Debug)]
pub(super) enum Event {
    RelativeMotion { dx_unaccel: f64, dy_unaccel: f64 },
    Active(bool),
}

macro_rules! proxy {
    ($ty:ident,$interface:ident) => {
        #[derive(Debug, Clone)]
        pub(super) struct $ty {
            id: ObjectId,
            version: u32,
            data: Option<Arc<dyn ObjectData>>,
            backend: WeakBackend,
        }
        impl PartialEq for $ty {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id
            }
        }
        impl Eq for $ty {}
        impl Proxy for $ty {
            type Request<'a> = Request<'a>;
            type Event = Event;
            fn interface() -> &'static Interface {
                &$interface
            }
            fn id(&self) -> ObjectId {
                self.id.clone()
            }
            fn version(&self) -> u32 {
                self.version
            }
            fn data<U: Send + Sync + 'static>(&self) -> Option<&U> {
                self.data
                    .as_ref()
                    .and_then(|d| d.data_as_any().downcast_ref())
            }
            fn object_data(&self) -> Option<&Arc<dyn ObjectData>> {
                self.data.as_ref()
            }
            fn backend(&self) -> &WeakBackend {
                &self.backend
            }
            fn send_request(&self, request: Self::Request<'_>) -> Result<(), InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                conn.send_request(self, request, None).map(|_| ())
            }
            fn send_constructor<I: Proxy>(
                &self,
                request: Self::Request<'_>,
                data: Arc<dyn ObjectData>,
            ) -> Result<I, InvalidId> {
                let conn = Connection::from_backend(self.backend.upgrade().ok_or(InvalidId)?);
                let id = conn.send_request(self, request, Some(data))?;
                I::from_id(&conn, id)
            }
            fn from_id(conn: &Connection, id: ObjectId) -> Result<Self, InvalidId> {
                if !id.is_null() && !same_interface(id.interface(), Self::interface()) {
                    return Err(InvalidId);
                }
                Ok(Self {
                    version: conn.object_info(id.clone()).map_or(0, |i| i.version),
                    data: conn.get_object_data(id.clone()).ok(),
                    backend: conn.backend().downgrade(),
                    id,
                })
            }
            fn inert(backend: WeakBackend) -> Self {
                Self {
                    id: ObjectId::null(),
                    version: 0,
                    data: None,
                    backend,
                }
            }
            fn parse_event(
                conn: &Connection,
                msg: Message<ObjectId, OwnedFd>,
            ) -> Result<(Self, Self::Event), DispatchError> {
                let me = Self::from_id(conn, msg.sender_id.clone()).map_err(|_| {
                    DispatchError::BadMessage {
                        sender_id: msg.sender_id.clone(),
                        interface: Self::interface().name,
                        opcode: msg.opcode,
                    }
                })?;
                if Self::interface().name == "zwp_relative_pointer_v1" && msg.opcode == 0 {
                    let mut args = msg.args.into_iter();
                    if let (
                        Some(Argument::Uint(_)),
                        Some(Argument::Uint(_)),
                        Some(Argument::Fixed(_)),
                        Some(Argument::Fixed(_)),
                        Some(Argument::Fixed(dx)),
                        Some(Argument::Fixed(dy)),
                    ) = (
                        args.next(),
                        args.next(),
                        args.next(),
                        args.next(),
                        args.next(),
                        args.next(),
                    ) {
                        return Ok((
                            me,
                            Event::RelativeMotion {
                                dx_unaccel: dx as f64 / 256.0,
                                dy_unaccel: dy as f64 / 256.0,
                            },
                        ));
                    }
                } else if msg.opcode < 2 && Self::interface().events.len() == 2 {
                    return Ok((me, Event::Active(msg.opcode == 0)));
                }
                Err(DispatchError::BadMessage {
                    sender_id: msg.sender_id,
                    interface: Self::interface().name,
                    opcode: msg.opcode,
                })
            }
            fn write_request<'a>(
                &self,
                _: &Connection,
                request: Self::Request<'a>,
            ) -> Result<
                (
                    Message<ObjectId, BorrowedFd<'a>>,
                    Option<(&'static Interface, u32)>,
                ),
                InvalidId,
            > {
                let mut message = Message {
                    sender_id: self.id(),
                    opcode: request.opcode,
                    args: Default::default(),
                };
                message.args.extend(request.args);
                Ok((message, request.child))
            }
        }
    };
}
proxy!(PointerConstraints, CONSTRAINTS);
proxy!(LockedPointer, LOCKED);
proxy!(RelativeManager, RELATIVE_MANAGER);
proxy!(RelativePointer, RELATIVE);

fn constructor<'a, I: Proxy>(
    opcode: u16,
    args: Vec<Argument<ObjectId, BorrowedFd<'a>>>,
) -> Request<'a> {
    Request {
        opcode,
        args,
        child: Some((I::interface(), 1)),
    }
}
fn destroy<P: for<'a> Proxy<Request<'a> = Request<'a>>>(p: &P) {
    let _ = p.send_request(Request {
        opcode: 0,
        args: Vec::new(),
        child: None,
    });
}
impl PointerConstraints {
    fn make<P: Proxy + 'static, U: Send + Sync + 'static, D: Dispatch<P, U> + 'static>(
        &self,
        opcode: u16,
        surface: &WlSurface,
        pointer: &WlPointer,
        region: Option<&WlRegion>,
        q: &QueueHandle<D>,
        data: U,
    ) -> P {
        let args = vec![
            Argument::NewId(ObjectId::null()),
            Argument::Object(surface.id()),
            Argument::Object(pointer.id()),
            Argument::Object(region.map_or_else(ObjectId::null, Proxy::id)),
            Argument::Uint(2),
        ];
        self.send_constructor(constructor::<P>(opcode, args), q.make_data::<P, U>(data))
            .unwrap_or_else(|_| P::inert(self.backend.clone()))
    }
    pub fn lock_pointer<U: Send + Sync + 'static, D: Dispatch<LockedPointer, U> + 'static>(
        &self,
        s: &WlSurface,
        p: &WlPointer,
        r: Option<&WlRegion>,
        q: &QueueHandle<D>,
        data: U,
    ) -> LockedPointer {
        self.make(1, s, p, r, q, data)
    }
}
impl LockedPointer {
    pub fn set_cursor_position_hint(&self, x: f64, y: f64) {
        let _ = self.send_request(Request {
            opcode: 1,
            args: vec![
                Argument::Fixed((x * 256.0) as i32),
                Argument::Fixed((y * 256.0) as i32),
            ],
            child: None,
        });
    }
    pub fn destroy(&self) {
        destroy(self);
    }
}
impl RelativeManager {
    pub fn get_relative_pointer<
        U: Send + Sync + 'static,
        D: Dispatch<RelativePointer, U> + 'static,
    >(
        &self,
        p: &WlPointer,
        q: &QueueHandle<D>,
        data: U,
    ) -> RelativePointer {
        self.send_constructor(
            constructor::<RelativePointer>(
                1,
                vec![Argument::NewId(ObjectId::null()), Argument::Object(p.id())],
            ),
            q.make_data::<RelativePointer, U>(data),
        )
        .unwrap_or_else(|_| RelativePointer::inert(self.backend.clone()))
    }
}
impl RelativePointer {
    pub fn destroy(&self) {
        destroy(self);
    }
}

// These descriptions cover the complete v1 wire interfaces, including requests the shell
// does not need to send. libwayland uses the C side to marshal objects and fixed-point values.
struct Messages(&'static [wl_message]);
unsafe impl Sync for Messages {}
macro_rules! interface {
    ($rust:ident,$c:ident,$name:literal,
     [$($rn:literal,$rs:literal,$rd:expr,[$($ra:expr),*],$child:expr,[$($objects:expr),*],[$($rt:expr),*];)*],
     [$($en:literal,$es:literal,[$($ea:expr),*],[$($et:expr),*];)*])=>{
        static $rust:Interface=Interface{name:$name,version:1,
            requests:&[$(MessageDesc{name:$rn,since:1,is_destructor:$rd,signature:&[$($ra),*],child_interface:$child,arg_interfaces:&[$($objects),*]}),*],
            events:&[$(MessageDesc{name:$en,since:1,is_destructor:false,signature:&[$($ea),*],child_interface:None,arg_interfaces:&[]}),*],c_ptr:Some(&$c)};
        static $c:wl_interface={
            static REQUESTS:Messages=Messages(&[$(wl_message{name:concat!($rn,"\0").as_ptr().cast(),signature:concat!($rs,"\0").as_ptr().cast(),types:[$($rt as *const wl_interface),*].as_ptr()}),*]);
            static EVENTS:Messages=Messages(&[$(wl_message{name:concat!($en,"\0").as_ptr().cast(),signature:concat!($es,"\0").as_ptr().cast(),types:[$($et as *const wl_interface),*].as_ptr()}),*]);
            wl_interface{name:concat!($name,"\0").as_ptr().cast(),version:1,request_count:REQUESTS.0.len()as i32,requests:REQUESTS.0.as_ptr(),event_count:EVENTS.0.len()as i32,events:EVENTS.0.as_ptr()}
        };
    };
}
use std::ptr::null;
interface!(CONSTRAINTS,CONSTRAINTS_C,"zwp_pointer_constraints_v1",[
    "destroy","",true,[],None,[],[];
    "lock_pointer","noo?ou",false,[A::NewId,A::Object(AllowNull::No),A::Object(AllowNull::No),A::Object(AllowNull::Yes),A::Uint],Some(&LOCKED),[&WL_SURFACE_INTERFACE,&WL_POINTER_INTERFACE,&WL_REGION_INTERFACE],[&LOCKED_C,&wl_surface_interface,&wl_pointer_interface,&wl_region_interface,null::<wl_interface>()];
    "confine_pointer","noo?ou",false,[A::NewId,A::Object(AllowNull::No),A::Object(AllowNull::No),A::Object(AllowNull::Yes),A::Uint],Some(&CONFINED),[&WL_SURFACE_INTERFACE,&WL_POINTER_INTERFACE,&WL_REGION_INTERFACE],[&CONFINED_C,&wl_surface_interface,&wl_pointer_interface,&wl_region_interface,null::<wl_interface>()];
],[]);
interface!(LOCKED,LOCKED_C,"zwp_locked_pointer_v1",[
    "destroy","",true,[],None,[],[];
    "set_cursor_position_hint","ff",false,[A::Fixed,A::Fixed],None,[],[null::<wl_interface>(),null::<wl_interface>()];
    "set_region","?o",false,[A::Object(AllowNull::Yes)],None,[&WL_REGION_INTERFACE],[&wl_region_interface];
],["locked","",[],[];"unlocked","",[],[];]);
interface!(CONFINED,CONFINED_C,"zwp_confined_pointer_v1",[
    "destroy","",true,[],None,[],[];
    "set_region","?o",false,[A::Object(AllowNull::Yes)],None,[&WL_REGION_INTERFACE],[&wl_region_interface];
],["confined","",[],[];"unconfined","",[],[];]);
interface!(RELATIVE_MANAGER,RELATIVE_MANAGER_C,"zwp_relative_pointer_manager_v1",[
    "destroy","",true,[],None,[],[];
    "get_relative_pointer","no",false,[A::NewId,A::Object(AllowNull::No)],Some(&RELATIVE),[&WL_POINTER_INTERFACE],[&RELATIVE_C,&wl_pointer_interface];
],[]);
interface!(RELATIVE,RELATIVE_C,"zwp_relative_pointer_v1",[
    "destroy","",true,[],None,[],[];
],["relative_motion","uuffff",[A::Uint,A::Uint,A::Fixed,A::Fixed,A::Fixed,A::Fixed],[null::<wl_interface>(),null::<wl_interface>(),null::<wl_interface>(),null::<wl_interface>(),null::<wl_interface>(),null::<wl_interface>()];]);
