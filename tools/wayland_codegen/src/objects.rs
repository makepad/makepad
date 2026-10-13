use crate::model::{camel, doc_attr, enum_type, ident, object_type, Interface, Protocol};
use crate::{enums, messages};

pub fn emit(protocol: &Protocol) -> String {
    let mut out = String::new();
    for i in &protocol.interfaces { object(&mut out,i); }
    out
}
fn object(out: &mut String, i: &Interface) {
    doc_attr(out,&i.doc);
    out.push_str(&format!("pub mod {} {{\nuse super::wayland_client;\nuse std::sync::Arc;\nuse std::os::fd::OwnedFd;\nuse wayland_client::{{Proxy, Connection, DispatchError, Weak}};\nuse wayland_client::backend::{{WeakBackend, ObjectData, ObjectId, InvalidId, protocol::{{Message, Interface, same_interface}}}};\n",i.name));
    if i.requests.iter().chain(&i.events).any(|m| !m.args.is_empty()) { out.push_str("use wayland_client::backend::protocol::Argument;\n"); }
    if i.requests.iter().chain(&i.events).any(|m| m.args.iter().any(|a|!a.enum_name.is_empty())) { out.push_str("use wayland_client::backend::protocol::WEnum;\n"); }
    for e in &i.enums { enums::emit(out,e); }
    messages::enums(out,i);
    let name=camel(&i.name);
    out.push_str(&PROXY.replace("@Name@",&name).replace("@INTERFACE@",&format!("{}_INTERFACE",i.name.to_ascii_uppercase())));
    messages::parser(out,i);
    messages::writer(out,i);
    out.push_str("}\n"); // Proxy impl
    out.push_str(&format!("impl {name} {{\n"));
    methods(out,i);
    out.push_str("}\n}\n");
}
fn methods(out: &mut String, i: &Interface) {
    for m in &i.requests {
        let constructor=m.args.iter().find(|a|a.kind=="new_id");
        let mut params: Vec<String>=Vec::new();
        let mut fields: Vec<String>=Vec::new();
        for a in &m.args {
            let n=ident(&a.name);
            if a.kind=="new_id" {
                if a.interface.is_empty(){params.push("version: u32".into());fields.push(format!("{n}: (I::interface(), version)"));}
                continue;
            }
            let mut ty=messages::field_type(a,false).expect("non-constructor argument");
            let mut expr=n.clone();
            if !a.enum_name.is_empty(){ty=enum_type(&a.enum_name);expr=format!("WEnum::Value({n})");}
            else if a.kind=="object" {
                let target=object_type(&a.interface);
                ty=if a.nullable{format!("Option<&{target}>")}else{format!("&{target}")};
                expr=if a.nullable{format!("{n}.cloned()")}else{format!("{n}.clone()")};
            } else if a.kind=="fd"{ty="std::os::fd::BorrowedFd<'_>".into();}
            params.push(format!("{n}: {ty}"));fields.push(format!("{n}: {expr}"));
        }
        let (generic,ret)=if let Some(a)=constructor {
            let ret=if a.interface.is_empty(){"I".into()}else{object_type(&a.interface)};
            let iface=if a.interface.is_empty(){"I: Proxy + 'static, "}else{""};
            params.push("qh: &wayland_client::QueueHandle<D>".into());params.push("udata: U".into());
            (format!("<{iface}U: Send + Sync + 'static, D: wayland_client::Dispatch<{ret}, U> + 'static>"),ret)
        } else {(String::new(),"()".into())};
        doc_attr(out,&m.doc);
        let comma=if params.is_empty(){""}else{", "};
        let variant=if m.args.is_empty(){camel(&m.name)}else{format!("{} {{ {} }}",camel(&m.name),fields.join(", "))};
        out.push_str(&format!("pub fn {}{generic}(&self{comma}{}) -> {ret} {{\n",ident(&m.name),params.join(", ")));
        if constructor.is_some() {
            out.push_str(&format!("    self.send_constructor::<{ret}>(Request::{variant}, qh.make_data::<{ret}, U>(udata)).unwrap_or_else(|_| {ret}::inert(self.backend.clone()))\n"));
        } else {
            out.push_str(&format!("    let Some(backend) = self.backend.upgrade() else {{ return; }};\n    let conn = Connection::from_backend(backend);\n    let _ = conn.send_request(self, Request::{variant}, None);\n"));
        }
        out.push_str("}\n");
    }
}

// These short generic methods implement the existing wayland-client Proxy
// contract. The runtime, queue dispatch and user-data ownership are unchanged.
const PROXY: &str = r#"
#[derive(Debug, Clone)]
pub struct @Name@ {
    id: ObjectId,
    version: u32,
    data: Option<Arc<dyn ObjectData>>,
    backend: WeakBackend,
}
impl PartialEq for @Name@ { fn eq(&self, other: &Self) -> bool { self.id == other.id } }
impl Eq for @Name@ {}
impl PartialEq<Weak<@Name@>> for @Name@ {
    fn eq(&self, other: &Weak<@Name@>) -> bool { self.id == other.id() }
}
impl std::borrow::Borrow<ObjectId> for @Name@ { fn borrow(&self) -> &ObjectId { &self.id } }
impl std::hash::Hash for @Name@ {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::hash::Hash::hash(&self.id, state); }
}
impl Proxy for @Name@ {
    type Request<'request> = Request<'request>;
    type Event = Event;
    #[inline]
    fn interface() -> &'static Interface { &super::@INTERFACE@ }
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
"#;
