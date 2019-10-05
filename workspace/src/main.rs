use hub::*;

fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    HubWorkspace::run(&key, "makepad", "./edit_repo", | workspace, htc | match htc.msg {
        HubMsg::CargoPackagesRequest {uid} => {
            workspace.cargo_packages(htc.from, uid, vec![
                HubCargoPackage::new("makepad", vec![
                    HubCargoTarget::Check,
                    HubCargoTarget::Release,
                    HubCargoTarget::IPC
                ])
            ])
        },
        _ => workspace.default(htc)
    });
}
