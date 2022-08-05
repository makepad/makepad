use makepad_ttf_parser;
use std::fs;
use std::path::Path;

#[test]
fn main() {
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().unwrap() != "ttf" {
            continue;
        }
        println!("{}", path.file_stem().unwrap().to_str().unwrap());
        let font = makepad_ttf_parser::parse_ttf(&fs::read(path).unwrap()).unwrap();
        for char_code in 0..font.char_code_to_glyph_index_map.len() {
            assert!(font.char_code_to_glyph_index_map[char_code] <= font.glyphs.len());
        }
    }
}
