
use {
    std::collections::HashMap,
    std::cell::RefCell,
    std::sync::{Mutex},
    std::ptr::NonNull,
    crate::{
        makepad_error_log::*,
        objc_block,
        objc_block_invoke,
        os::{
            apple::frameworks::*,
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
        },
    }
};

static mut METAL_XPC_CLASSES: *const MetalXPCClasses = 0 as *const _;
static mut METAL_XPC_STORAGE: *const MetalXPCStorage = 0 as *const _;
fn get_metal_xpc_classes() -> &'static MetalXPCClasses {unsafe {&*(METAL_XPC_CLASSES)}}
fn get_metal_xpc_storage() -> &'static MetalXPCStorage {unsafe {&*(METAL_XPC_STORAGE)}}

struct MetalXPCClasses {
    pub xpc_service_delegate: *const Class,
    pub xpc_service_protocol: *const Protocol,
    pub xpc_service_class: *const Class,
}

#[derive(Default)]
struct MetalXPCStorage {
    pub textures: Mutex<RefCell<HashMap<u64, (RcObjcId,u64) >> >
}

impl MetalXPCClasses {
    pub fn new() -> Self {
        Self {
            xpc_service_delegate: define_xpc_service_delegate(),
            xpc_service_protocol: unsafe {define_xpc_service_protocol()},
            xpc_service_class: define_xpc_service_class(),
        }
    }
}

extern {
    fn define_xpc_service_protocol() -> &'static Protocol;
    fn get_fetch_texture_completion_block(data: u64) -> ObjcId;
    fn get_store_completion_block(data: u64) -> ObjcId;
}

pub fn xpc_service_proxy_poll_run_loop() {
    unsafe {
        let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
        let nsuntil: ObjcId = msg_send![class!(NSDate), distantPast];
        let () = msg_send![nsrunloop, runUntilDate: nsuntil];
    }
}

pub fn xpc_service_proxy() -> RcObjcId {
    unsafe {
        if METAL_XPC_CLASSES == 0 as *const _ {
            METAL_XPC_CLASSES = Box::into_raw(Box::new(MetalXPCClasses::new()));
        }
        let connection: ObjcId = msg_send![class!(NSXPCConnection), new];
        let nsstring = str_to_nsstring("dev.makepad.metalxpc");
        let () = msg_send![connection, initWithMachServiceName: nsstring options: 0];
        let iface: ObjcId = msg_send![
            class!(NSXPCInterface),
            interfaceWithProtocol: get_metal_xpc_classes().xpc_service_protocol
        ];
        let () = msg_send![connection, setRemoteObjectInterface: iface];
        let () = msg_send![connection, resume];
        
        let error_handler = objc_block!(move | error: ObjcId | {
            let desc: ObjcId = msg_send![error, localizedDescription];
            log!("xpc_service_proxy got error: {}", nsstring_to_string(desc));
        });
        let proxy: ObjcId = msg_send![connection, remoteObjectProxyWithErrorHandler: error_handler];
        RcObjcId::from_unowned(NonNull::new(proxy).unwrap())
    }
}

pub fn fetch_xpc_service_texture(proxy: ObjcId, id: u64, uid:u64, f: Box<dyn Fn(RcObjcId, u64)>) {
    unsafe {
        let texture_out = objc_block!(move | texture: ObjcId, uid: u64 | {
            f(RcObjcId::from_unowned(NonNull::new(texture).unwrap()), uid)
        });
        let completion_block = get_fetch_texture_completion_block(&texture_out as *const _ as u64);
        let () = msg_send![proxy, fetchTexture: id uid:uid with: completion_block];
        //log!("FETCH SERVICE TEXTURE");
    } 
}


pub fn store_xpc_service_texture(id: u64, obj: ObjcId) {
    //log!("STORING {}", obj as *const _ as u64);
    unsafe {
        let proxy = xpc_service_proxy();
        let texture_out = objc_block!(move | _texture: ObjcId | {
            //log!("store texture complete!");
        });
        let completion_block = get_store_completion_block(&texture_out as *const _ as u64);
        let uid = obj as * const _ as u64;
        let () = msg_send![proxy.as_id(), storeTexture: id obj: obj uid: uid with: completion_block];
    }
}

use std::io::prelude::*;

pub fn insane_debug_out(what: &str) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open("/Users/admin/makepad/edit_repo/insane.log")
        .unwrap();
    
    let _ = file.write_all(format!("{}\n", what).as_bytes());
}

pub fn start_xpc_service() {
    unsafe {
        //insane_debug_out("START XPC SERVICE");
        METAL_XPC_CLASSES = Box::into_raw(Box::new(MetalXPCClasses::new()));
        METAL_XPC_STORAGE = Box::into_raw(Box::new(MetalXPCStorage::default()));
        let delegate: ObjcId = msg_send![get_metal_xpc_classes().xpc_service_delegate, new];
        let nsstring = str_to_nsstring("dev.makepad.metalxpc");
        let listener: ObjcId = msg_send![class!(NSXPCListener), new];
        let () = msg_send![listener, setDelegate: delegate];
        let () = msg_send![listener, initWithMachServiceName: nsstring];
        let () = msg_send![listener, activate];
        let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
        let () = msg_send![nsrunloop, run];
        //insane_debug_out("STOP XPC SERVICE");
    }
}

pub fn define_xpc_service_delegate() -> *const Class {
    
    extern fn listener(_this: &Object, _: Sel, _listener: ObjcId, connection: ObjcId) -> bool {
        //insane_debug_out("LISTENER INCOMING");
        unsafe {
            let iface: ObjcId = msg_send![
                class!(NSXPCInterface),
                interfaceWithProtocol: get_metal_xpc_classes().xpc_service_protocol
            ];
            
            let api: ObjcId = msg_send![get_metal_xpc_classes().xpc_service_class, new];
            
            let () = msg_send![connection, setExportedInterface: iface];
            let () = msg_send![connection, setExportedObject: api];
            let () = msg_send![connection, resume];
        }
        true
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MetalXPCServiceDelegate", superclass).unwrap();
    
    unsafe {
        decl.add_method(sel!(listener: shouldAcceptNewConnection:), listener as extern fn(&Object, Sel, ObjcId, ObjcId) -> bool);
    }
    
    return decl.register();
}

pub fn define_xpc_service_class() -> *const Class {
    
    extern fn fetch_texture(_this: &Object, _: Sel, index: u64, old_uid: u64, with: ObjcId) {
        let storage = get_metal_xpc_storage();
        if let Some((obj, uid)) = storage.textures.lock().unwrap().borrow_mut().get(&index) {
            if *uid != old_uid{
                unsafe {objc_block_invoke!(with, invoke(
                    (obj.as_id()): ObjcId,
                    (*uid): u64
                ))}; 
            }
        } 
        //insane_debug_out("GOT CALL! POST FETCH TEXTURE!");
    }
     
    extern fn store_texture(_this: &Object, _: Sel, index: u64, obj: ObjcId, uid:u64, with: ObjcId) {
        let storage = get_metal_xpc_storage();
        storage.textures.lock().unwrap().borrow_mut().insert(
            index,
            (RcObjcId::from_unowned(NonNull::new(obj).unwrap()), uid)
        );
        unsafe {objc_block_invoke!(with, invoke());}
        //insane_debug_out(&format!("STORE TEXTURE! {}", index));
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MetalXPCServiceAPI", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(fetchTexture: uid: with:), fetch_texture as extern fn(&Object, Sel, u64, u64, ObjcId));
        decl.add_method(sel!(storeTexture: obj: uid: with:), store_texture as extern fn(&Object, Sel, u64, ObjcId, u64, ObjcId));
        decl.add_protocol(define_xpc_service_protocol());
    }
    
    return decl.register();
}
