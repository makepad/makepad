use hub::*;

fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, | ws, htc | match htc.msg {
        HubMsg::CargoPackagesRequest {uid} => {
            ws.cargo_packages(htc.from, uid, vec![
                HubCargoPackage::new("makepad", &["check", "makepad", "workspace"])
            ])
        },
        HubMsg::CargoExec {uid, package, target} => {
            match package.as_ref() {
                "makepad" => match target.as_ref() {
                    "check" => ws.cargo_exec(uid, &["check", "-p", &package]),
                    "makepad" => ws.cargo_exec(uid, &["build", "-p", "makepad"]),
                    "workspace" => ws.cargo_exec(uid, &["build", "-p", "workspace"]),
                    _ => ()
                },
                _ => ()
            }
        },
        _ => ws.default(htc)
    });
}
