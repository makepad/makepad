// builders are networked build and file servers. This 'main' one is also compiled into makepad

#[path = "../../makepad/src/builder.rs"]
mod builder;
use hub::*;

#[allow(dead_code)]
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    HubBuilder::run_builder_commandline(args, | ws, htc | {builder::builder(ws, htc)});
}
