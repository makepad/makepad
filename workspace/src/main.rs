use hub::*;

fn main() {
    let key = [7u8, 4u8, 5u8, 1u8];
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
