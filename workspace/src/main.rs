use workspacelib::*;

fn main() {
    Workspace::run("makepad", | workspace, htc | match htc.msg {
        HubMsg::CargoTargetsRequest {uid} => {
            workspace.cargo_targets(uid, &["makepad"])
        },
        _ => workspace.default(htc)
    });
}
