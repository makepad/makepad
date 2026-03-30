use crate::cx::Cx;
use crate::event::Event;
use crate::ui_runner::UiRunner;
use makepad_script::{ScriptValue, ScriptVm};

#[cfg(target_env = "ohos")]
pub use napi_ohos;

pub fn should_run_stdin_loop_from_env() -> bool {
    std::env::args().any(|v| v == "--stdin-loop")
        || std::env::var("MAKEPAD_STDIN_LOOP").is_ok_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

#[cfg(headless)]
pub(crate) fn should_disable_headless_draw_from_args() -> bool {
    std::env::args().any(|v| v == "--no-draw")
}

#[cfg(headless)]
pub(crate) fn headless_draw_cycles_from_args() -> Option<usize> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--draws=") {
            if let Ok(draws) = value.parse::<usize>() {
                return Some(draws.max(1));
            }
        }
        if arg == "--draws" {
            if let Some(value) = args.next() {
                if let Ok(draws) = value.parse::<usize>() {
                    return Some(draws.max(1));
                }
            }
        }
    }
    None
}

fn normalize_studio_http_from_studio_var(studio: &str) -> String {
    let studio = studio.trim().trim_end_matches('/');
    if studio.is_empty() {
        return String::new();
    }

    if studio.contains("://") {
        studio.to_string()
    } else {
        format!("http://{studio}")
    }
}

#[cfg(any(test, target_os = "linux"))]
pub(crate) fn extract_studio_build_id(studio: &str) -> Option<String> {
    let studio = studio.trim().trim_end_matches('/');
    if studio.is_empty() {
        return None;
    }

    let without_scheme = studio
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(studio);
    let path = without_scheme
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or("");

    if let Some(rest) = path.strip_prefix("app/") {
        let build_id = rest.split('/').next()?;
        if !build_id.is_empty() {
            return Some(build_id.to_string());
        }
    }

    None
}

pub fn resolve_studio_http() -> String {
    std::env::var("STUDIO")
        .ok()
        .map(|studio| normalize_studio_http_from_studio_var(&studio))
        .filter(|studio_http| !studio_http.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_studio_build_id_handles_app_paths() {
        assert_eq!(
            extract_studio_build_id("127.0.0.1:8001/app/42"),
            Some("42".to_string())
        );
        assert_eq!(
            extract_studio_build_id("http://127.0.0.1:8001/app/77"),
            Some("77".to_string())
        );
    }
}

pub trait AppMain {
    fn script_mod(_vm: &mut ScriptVm) -> ScriptValue
    where
        Self: Sized,
    {
        panic!("AppMain::script_mod not implemented for this app")
    }

    fn after_new_from_script(_vm: &mut ScriptVm, _app: &mut Self)
    where
        Self: Sized,
    {
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event);
    fn ui_runner(&self) -> UiRunner<Self>
    where
        Self: Sized + 'static,
    {
        // This assumes there is only one `AppMain`, and that `0` is reserved for it.
        UiRunner::new(0)
    }
}

#[macro_export]
macro_rules! app_main {
    ( $ app: ident) => {
        #[cfg(not(any(target_os = "android", target_env = "ohos")))]
        fn main() {
            app_main();
        }

        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
        pub fn app_main() {
            Cx::init_log();
            if Cx::pre_start() {
                return;
            }

            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = std::rc::Rc::new(std::cell::RefCell::new(Cx::new(Box::new(
                move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| {
                            let value = <$app as AppMain>::script_mod(vm);
                            let mut app = <$app as $crate::ScriptNew>::script_from_value(vm, value);
                            <$app as AppMain>::after_new_from_script(vm, &mut app);
                            app
                        }));
                        cx.start_hot_reload_file_observer_if_requested();
                    }
                    if let Event::LiveEdit = event {
                        let mut app_ref = app.borrow_mut();
                        if let Some(app) = app_ref.as_mut() {
                            cx.with_vm(|vm| {
                                let value = vm.with_reload(|vm| <$app as AppMain>::script_mod(vm));
                                <$app as $crate::ScriptApply>::script_apply(
                                    app,
                                    vm,
                                    &$crate::Apply::Reload,
                                    &mut $crate::Scope::empty(),
                                    value,
                                );
                            });
                        }
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                },
            ))));
            let studio_http = $crate::resolve_studio_http();
            cx.borrow_mut().init_websockets(&studio_http);
            if $crate::should_run_stdin_loop_from_env() {
                cx.borrow_mut().in_makepad_studio = true;
            }
            //cx.borrow_mut().init_websockets("");
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
            Cx::init_log();
            $crate::os::linux::android::android_jni::apply_studio_env_from_activity(activity);
            Cx::android_entry(activity, || {
                let studio_http = $crate::resolve_studio_http();
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| {
                            let value = <$app as AppMain>::script_mod(vm);
                            let mut app = <$app as $crate::ScriptNew>::script_from_value(vm, value);
                            <$app as AppMain>::after_new_from_script(vm, &mut app);
                            app
                        }));
                        cx.start_hot_reload_file_observer_if_requested();
                    }
                    if let Event::LiveEdit = event {
                        let mut app_ref = app.borrow_mut();
                        if let Some(app) = app_ref.as_mut() {
                            cx.with_vm(|vm| {
                                let value = vm.with_reload(|vm| <$app as AppMain>::script_mod(vm));
                                <$app as $crate::ScriptApply>::script_apply(
                                    app,
                                    vm,
                                    &$crate::Apply::Reload,
                                    &mut $crate::Scope::empty(),
                                    value,
                                );
                            });
                        }
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                })));
                cx.init_websockets(&studio_http);
                cx.init_cx_os();
                cx
            })
        }

        #[cfg(target_env = "ohos")]
        #[no_mangle]
        extern "C" fn ohos_init_app_main(
            exports: $crate::napi_ohos::JsObject,
            env: $crate::napi_ohos::Env,
        ) -> $crate::napi_ohos::Result<()> {
            Cx::ohos_init(exports, env, || {
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| {
                            let value = <$app as AppMain>::script_mod(vm);
                            let mut app = <$app as $crate::ScriptNew>::script_from_value(vm, value);
                            <$app as AppMain>::after_new_from_script(vm, &mut app);
                            app
                        }));
                        cx.start_hot_reload_file_observer_if_requested();
                    }
                    if let Event::LiveEdit = event {
                        let mut app_ref = app.borrow_mut();
                        if let Some(app) = app_ref.as_mut() {
                            cx.with_vm(|vm| {
                                let value = vm.with_reload(|vm| <$app as AppMain>::script_mod(vm));
                                <$app as $crate::ScriptApply>::script_apply(
                                    app,
                                    vm,
                                    &$crate::Apply::Reload,
                                    &mut $crate::Scope::empty(),
                                    value,
                                );
                            });
                        }
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                })));
                let studio_http = $crate::resolve_studio_http();
                cx.init_websockets(&studio_http);
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
            Cx::init_log();
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                if let Event::Startup = event {
                    *app.borrow_mut() = Some(cx.with_vm(|vm| {
                        let value = <$app as AppMain>::script_mod(vm);
                        let mut app = <$app as $crate::ScriptNew>::script_from_value(vm, value);
                        <$app as AppMain>::after_new_from_script(vm, &mut app);
                        app
                    }));
                }
                if let Event::LiveEdit = event {
                    let mut app_ref = app.borrow_mut();
                    if let Some(app) = app_ref.as_mut() {
                        cx.with_vm(|vm| {
                            let value = vm.with_reload(|vm| <$app as AppMain>::script_mod(vm));
                            <$app as $crate::ScriptApply>::script_apply(
                                app,
                                vm,
                                &$crate::Apply::Reload,
                                &mut $crate::Scope::empty(),
                                value,
                            );
                        });
                    }
                }
                if let Some(app) = &mut *app.borrow_mut() {
                    <dyn AppMain>::handle_event(app, cx, event);
                }
            })));
            let studio_http = $crate::resolve_studio_http();
            cx.init_websockets(&studio_http);
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
    };
}

#[cfg(target_env = "ohos")]
#[napi_derive_ohos::module_exports]
fn init(exports: napi_ohos::JsObject, env: napi_ohos::Env) -> napi_ohos::Result<()> {
    #[allow(improper_ctypes)]
    extern "C" {
        fn ohos_init_app_main(
            exports: napi_ohos::JsObject,
            env: napi_ohos::Env,
        ) -> napi_ohos::Result<()>;
    }
    unsafe { ohos_init_app_main(exports, env) }
}
