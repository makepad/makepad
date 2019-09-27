use makelib::*;

fn main() {
    Make::proc( | make, msg | match msg {
        HubMsg::CargoCheck => {
            make.cargo("check -p makepad")
        }
    });
}
