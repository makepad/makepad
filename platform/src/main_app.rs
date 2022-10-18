
#[macro_export]
macro_rules!main_app {
    ( $ app: ident) => {
        #[cfg(not(target_arch = "wasm32"))]
        fn main() {
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Cx::new(Box::new(move | cx, event | {
                
                if let Event::Construct = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                }
                
                app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
            }));
            live_design(&mut cx);
            cx.init();
            cx.event_loop();
        }
        
        #[cfg(target_arch = "wasm32")]
        fn main() {}
        
        #[export_name = "wasm_create_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                if let Event::Construct = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                }
                app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
            })));
            
            live_design(&mut cx);
            cx.init();
            Box::into_raw(cx) as u32
        }

        #[export_name = "wasm_process_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_process_msg(msg_ptr: u32, cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.process_to_wasm(msg_ptr)
        }
    }
}

