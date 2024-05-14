use std::env;
use std::fs::File;
use std::path::Path;
use std::io::prelude::*;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir).parent().unwrap().parent().unwrap().parent().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-widgets.path")).unwrap();
    file.write_all(&format!("{}", cwd.display()).as_bytes()).unwrap();
}