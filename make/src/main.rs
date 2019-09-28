use makelib::*;

fn main() {
    Make::proc( | make, htc | match htc.msg {
        HubMsg::GetCargoTargets {uid} => {
            make.cargo_has_targets(uid, &["makepad"])
        },
        _ => false
    });
}
