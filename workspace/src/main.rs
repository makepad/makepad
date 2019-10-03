use hub::*;

fn main() {
    HubWorkspace::run("makepad", ".", | workspace, htc | match htc.msg {
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
