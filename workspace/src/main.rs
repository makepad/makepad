use hub::*;

fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    HubWorkspace::run(&key, "makepad", "./edit_repo", | workspace, htc | match htc.msg {
        HubMsg::CargoPackagesRequest {uid} => {
            workspace.cargo_packages(htc.from, uid, vec![
                CargoPackage::new("makepad", vec![
                    CargoTarget::Check,
                    CargoTarget::Release,
                    CargoTarget::IPC
                ])
            ])
        },
        _ => workspace.default(htc)
    });
}
