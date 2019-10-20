use hub::*;

pub fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, | ws, htc | match htc.msg {
        HubMsg::CargoPackagesRequest {uid} => {
            let targets = &["check","debug","release"];
            ws.cargo_packages(htc.from, uid, vec![
                HubCargoPackage::new("workspace", targets),
                HubCargoPackage::new("makepad", targets),
                HubCargoPackage::new("csvproc", targets),
                HubCargoPackage::new("ui_example", targets),
            ]);
        },
        HubMsg::CargoExec {uid, package, target}=>{
            match target.as_ref(){
                "release"=>{
                    ws.cargo_exec(uid, &["build", "--release", "-p", &package], &[])
                },
                "debug"=>{
                    ws.cargo_exec(uid, &["build", "-p", &package], &[])
                },
                "check"=>{
                    ws.cargo_exec(uid, &["check", "-p", &package], &[])
                },
                _=>ws.cargo_exec_fail(uid, &package, &target)
            }
        },
        _ => ws.default(htc)
    });
}
