
use crate::event::Event;
use crate::cx::Cx;

pub trait AppMain{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event);
}

#[macro_export]
macro_rules!app_main {
    ( $ app: ident) => {
        #[cfg(not(any(target_arch = "wasm32", target_os="android", target_env="ohos")))]
        pub fn app_main() {
            if Cx::pre_start(){
                return
            }
                        
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = std::rc::Rc::new(std::cell::RefCell::new(Cx::new(Box::new(move | cx, event | {
                if let Event::Startup = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                }
                if let Event::LiveEdit = event{
                    app.borrow_mut().update_main(cx);
                }
                <dyn AppMain>::handle_event(app.borrow_mut().as_mut().unwrap(), cx, event);
            }))));
            $app::register_main_module(&mut *cx.borrow_mut());
            cx.borrow_mut().init_websockets(std::option_env!("MAKEPAD_STUDIO_HTTP").unwrap_or(""));
            //cx.borrow_mut().init_websockets("");
            live_design(&mut *cx.borrow_mut());
            cx.borrow_mut().init_cx_os();
            Cx::event_loop(cx);
        }
        
        /*
        #[cfg(target_os = "android")]
        #[no_mangle]
        pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onNewCx(_: *const std::ffi::c_void, _: *const std::ffi::c_void) -> i64 {
            Cx::android_entry(||{
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                    if let Event::Construct = event {
                        *app.borrow_mut() = Some($app::new_main(cx));
                    }
                    if let Event::LiveEdit = event{
                        app.borrow_mut().update_main(cx);
                    }
                    app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                })));
                live_design(&mut cx);
                cx.init_cx_os();
                cx
            })
        }*/

        
        #[cfg(target_os = "android")]
        #[no_mangle]
        pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnCreate(
            _: *const std::ffi::c_void,
            _: *const std::ffi::c_void,
            activity: *const std::ffi::c_void,
        ) {
            Cx::android_entry(activity, ||{
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some($app::new_main(cx));
                    }
                    if let Event::LiveEdit = event{
                        app.borrow_mut().update_main(cx);
                    }
                    app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                })));
                $app::register_main_module(&mut cx);
                cx.init_websockets(std::option_env!("MAKEPAD_STUDIO_HTTP").unwrap_or(""));
                live_design(&mut cx);
                cx.init_cx_os();
                cx
            })
        }

        #[cfg(target_env = "ohos")]
        #[napi_derive_ohos::module_exports]
        fn init(exports: napi_ohos::JsObject, env: napi_ohos::Env) -> napi_ohos::Result<()> {
            Cx::ohos_init(exports,env, ||{
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some($app::new_main(cx));
                    }
                    if let Event::LiveEdit = event{
                        app.borrow_mut().update_main(cx);
                    }
                    app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                })));
                $app::register_main_module(&mut cx);
                cx.init_websockets(std::option_env!("MAKEPAD_STUDIO_HTTP").unwrap_or(""));
                live_design(&mut cx);
                cx.init_cx_os();
                cx
            });
            Ok(())
        }
        
        #[cfg(target_arch = "wasm32")]
        pub fn app_main() {}
        
        #[export_name = "wasm_create_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                if let Event::Startup = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                }
                if let Event::LiveEdit = event{
                    app.borrow_mut().update_main(cx);
                }
                app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
            })));
            $app::register_main_module(&mut cx);
            cx.init_websockets(std::option_env!("MAKEPAD_STUDIO_HTTP").unwrap_or(""));
            live_design(&mut cx);
            cx.init_cx_os();
            Box::into_raw(cx) as u32
        }
        
                
        #[export_name = "wasm_process_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_process_msg(msg_ptr: u32, cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.process_to_wasm(msg_ptr)
        }
        
        #[export_name = "wasm_return_first_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_return_first_msg(cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx); 
            cx.os.from_wasm.take().unwrap().release_ownership()
        }
        
    }
}

