
mod malloc_buf;

pub use encode::{Encode, EncodeArguments, Encoding};
pub use message::{Message, MessageArguments, MessageError};

pub use message::send_message as __send_message;
pub use message::send_super_message as __send_super_message;

#[macro_use]
mod macros;

pub mod runtime;
pub mod declare;
pub mod rc;
mod encode;
mod message;

//#[cfg(test)]
//mod test_utils;



#[macro_export]
macro_rules!objc_block {
    (move | $ ( $ arg_ident: ident: $ arg_ty: ty), * | $ (: $ return_ty: ty) ? $ body: block) => {
        {
            #[repr(C)]
            struct BlockDescriptor {
                reserved: std::os::raw::c_ulong,
                size: std::os::raw::c_ulong,
                copy_helper: extern "C" fn(*mut std::os::raw::c_void, *const std::os::raw::c_void),
                dispose_helper: extern "C" fn(*mut std::os::raw::c_void),
            }
            
            static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
                reserved: 0,
                size: std::mem::size_of::<BlockLiteral>() as std::os::raw::c_ulong,
                copy_helper,
                dispose_helper,
            };
            
            #[allow(unused_unsafe)]
            extern "C" fn copy_helper(dst: *mut std::os::raw::c_void, src: *const std::os::raw::c_void) {
                unsafe {
                    std::ptr::write(
                        &mut (*(dst as *mut BlockLiteral)).inner as *mut _,
                        (&*(src as *const BlockLiteral)).inner.clone()
                    );
                }
            }
            
            #[allow(unused_unsafe)]
            extern "C" fn dispose_helper(src: *mut std::os::raw::c_void) {
                unsafe {
                    std::ptr::drop_in_place(src as *mut BlockLiteral);
                }
            }
            
            #[allow(unused_unsafe)]
            extern "C" fn invoke(literal: *mut BlockLiteral, $ ( $ arg_ident: $ arg_ty), *) $ ( -> $ return_ty) ? {
                let literal = unsafe {&mut *literal};
                literal.inner.lock().unwrap()( $ ( $ arg_ident), *)
            }
            
            #[repr(C)]
            struct BlockLiteral {
                isa: *const std::os::raw::c_void,
                flags: std::os::raw::c_int,
                reserved: std::os::raw::c_int,
                invoke: extern "C" fn(*mut BlockLiteral, $ ( $ arg_ty), *) $ ( -> $ return_ty) ?,
                descriptor: *const BlockDescriptor,
                inner: ::std::sync::Arc<::std::sync::Mutex<dyn Fn( $ ( $ arg_ty), *) $ ( -> $ return_ty) ? >>,
            }
            
            #[allow(unused_unsafe)]
            BlockLiteral {
                isa: unsafe {_NSConcreteStackBlock.as_ptr() as *const std::os::raw::c_void},
                flags: 1 << 25,
                reserved: 0,
                invoke,
                descriptor: &DESCRIPTOR,
                inner: ::std::sync::Arc::new(::std::sync::Mutex::new(move | $ ( $ arg_ident: $ arg_ty), * | {
                    $ body
                }))
            }
        }
    }
}



#[macro_export]
macro_rules!objc_block_invoke {
    ( $ inp: expr, invoke ( $ ( ($ arg_ident: expr): $ arg_ty: ty), *) $ ( -> $ return_ty: ty) ?) => {
        {
            #[repr(C)]
            struct BlockLiteral {
                isa: *const std::os::raw::c_void,
                flags: std::os::raw::c_int,
                reserved: std::os::raw::c_int,
                invoke: extern "C" fn(*mut BlockLiteral, $ ( $ arg_ty), *) $ ( -> $ return_ty) ?,
            }
            
            let block: &mut BlockLiteral = &mut *( $ inp as *mut _);
            (block.invoke)(block, $ ( $ arg_ident), *)
        }
    }
}
