use hub::*;

pub fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    
    HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, HttpServe::Local(2001), | ws, htc | match htc.msg {
        HubMsg::CargoPackagesRequest {uid} => {
            let builds = &["check", "debug", "release"];
            ws.cargo_packages(htc.from, uid, vec![
                HubCargoPackage::new("workspace", builds),
                HubCargoPackage::new("makepad", builds),
                HubCargoPackage::new("csvproc", builds),
                HubCargoPackage::new("ui_example", builds),
                HubCargoPackage::new("makepad_webgl", builds),
            ]);
        },
        HubMsg::CargoExec {uid, package, build} => {
            let mut args = Vec::new();
            let mut env = Vec::new();
            match build.as_ref() {
                "release" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                "debug" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                "check" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                _ => return ws.cargo_exec_fail(uid, &package, &build)
            }
            match package.as_ref() {
                "makepad_webgl" => args.push("--target=wasm32-unknown-unknown"),
                _ => ()
            }
            ws.cargo_exec(uid, &args, &env);
        },
        _ => ws.default(htc)
    });
}
