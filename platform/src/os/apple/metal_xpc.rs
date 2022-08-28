
use {
    crate::{
        objc_block,
        objc_block_invoke,
        os::{
            apple::frameworks::*,
            apple_util::{
                str_to_nsstring,
            },
        },
    }
};

// this is all needed to get a texture handle from process A to process B

static mut METAL_XPC_CLASSES: *const MetalXPCClasses = 0 as *const _;

struct MetalXPCClasses {
    pub xpc_service_delegate: *const Class,
    pub xpc_service_protocol: *const Protocol,
    pub xpc_service_class: *const Class,
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

fn get_metal_xpc_classes() -> &'static MetalXPCClasses {
    unsafe {
        &*(METAL_XPC_CLASSES)
    }
}

pub fn fetch_xpc_service_texture(id: u64) {
    unsafe {
        METAL_XPC_CLASSES = Box::into_raw(Box::new(MetalXPCClasses::new()));
        let connection: ObjcId = msg_send![class!(NSXPCConnection), new];
        let nsstring = str_to_nsstring("dev.makepad.metalxpc");
        let () = msg_send![connection, initWithMachServiceName: nsstring options: 0];
        
        let iface: ObjcId = msg_send![
            class!(NSXPCInterface),
            interfaceWithProtocol: get_metal_xpc_classes().xpc_service_protocol
        ];
        
        let () = msg_send![connection, setRemoteObjectInterface: iface];
        let () = msg_send![connection, resume];
        let proxy: ObjcId = msg_send![connection, remoteObjectProxy];
        
        let completion_block = get_xpc_completion_block();
        insane_debug_out("FETCHING TEXTURE");
        let () = msg_send![proxy, fetchTexture: id with: completion_block];
        let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
        let () = msg_send![nsrunloop, run];
    }
}

use std::io::prelude::*;

pub fn insane_debug_out(what:&str){
    let mut file = std::fs::OpenOptions::new()
      .write(true)
      .append(true)
      .open("/Users/admin/makepad/edit_repo/insane.log")
      .unwrap();
      
    let _ = file.write_all(format!("{}\n",what).as_bytes());    
}

pub fn start_xpc_service() { 
    unsafe {
        insane_debug_out("START XPC SERVICE");
        METAL_XPC_CLASSES = Box::into_raw(Box::new(MetalXPCClasses::new()));
        let delegate: ObjcId = msg_send![get_metal_xpc_classes().xpc_service_delegate, new];
        let nsstring = str_to_nsstring("dev.makepad.metalxpc");
        let listener: ObjcId = msg_send![class!(NSXPCListener), new];
        let () = msg_send![listener, setDelegate: delegate];
        let () = msg_send![listener, initWithMachServiceName: nsstring];
        let () = msg_send![listener, activate]; 
        let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
        let () = msg_send![nsrunloop, run];
    }
}

pub fn define_xpc_service_delegate() -> *const Class {
    
    extern fn listener(_this: &Object, _: Sel, _listener:ObjcId, connection: ObjcId) -> bool {
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
    let mut decl = ClassDecl::new("ServiceDelegate", superclass).unwrap();
    
    unsafe {
        decl.add_method(sel!(listener: shouldAcceptNewConnection:), listener as extern fn(&Object, Sel, ObjcId, ObjcId) -> bool);
    }
    
    return decl.register();
}

extern {
    fn define_xpc_service_protocol() -> &'static Protocol;
    fn get_xpc_completion_block() -> ObjcId;
}

pub fn define_xpc_service_class() -> *const Class {
    
    extern fn fetch_texture(_this: &Object, _: Sel, _index: u64, with: ObjcId) {
        unsafe {
            objc_block_invoke!(with, invoke(
                (nil): ObjcId
            ));
        }
        insane_debug_out("GOT CALL! FETCH TEXTURE!");
    }
    
    extern fn store_texture(_this: &Object, _: Sel, _index: u64, _obj: ObjcId) {
        insane_debug_out("STORE TEXTURE!");
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("ServiceAPI", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(fetchTexture: with:), fetch_texture as extern fn(&Object, Sel, u64, ObjcId));
        decl.add_method(sel!(storeTexture: obj:), store_texture as extern fn(&Object, Sel, u64, ObjcId));
        decl.add_protocol(define_xpc_service_protocol());
    }
    
    return decl.register();
}
