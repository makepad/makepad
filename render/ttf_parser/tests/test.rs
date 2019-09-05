use ttf_parser;
use std::fs;
use std::path::Path;

#[test]
fn main() {
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().unwrap() != "ttf" {
            continue;
        }
        println!("{}", path.file_stem().unwrap().to_str().unwrap());
        ttf_parser::parse_ttf(&fs::read(path).unwrap()).unwrap();
    }
}
