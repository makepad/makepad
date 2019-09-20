use makelib::*;

fn main() {
    Make::proc( | make, cmd | match cmd {
        MakeCmd::Check => {
            make.cargo("check -p makepad")
        }
    });
}
