use {
    rustybuzz::{ttf_parser, ttf_parser::GlyphId, UnicodeBuffer},
    resvg::{tiny_skia::{Pixmap, Transform}, usvg::{fontdb::Database, Options, Tree}},
};

fn main() {
    let data = include_bytes!("NotoColorEmoji-Regular.ttf");
    let ttf_parser_face = ttf_parser::Face::parse(data, 0).unwrap();
    let rustybuzz_face = rustybuzz::Face::from_face(ttf_parser_face.clone());
    let mut unicode_buffer = UnicodeBuffer::new();
    unicode_buffer.push_str("\u{2764}\u{FE0F}\u{200D}\u{1F525}");
    let glyph_buffer = rustybuzz::shape(&rustybuzz_face, &[], unicode_buffer);
    let glyph_id = glyph_buffer.glyph_infos()[0].glyph_id;
    let svg_document = ttf_parser_face.glyph_svg_image(GlyphId(glyph_id as u16)).unwrap();
    let opt = Options::default();
    let fontdb = Database::new();
    let tree = Tree::from_data(
        &svg_document.data,
        &opt,
        &fontdb,
    ).unwrap();
    let id = format!("glyph{}", glyph_id);
    let node = tree.node_by_id(&id).unwrap();
    let mut pixmap = Pixmap::new(1024, 1024).unwrap();
    resvg::render_node(&node, Transform::identity(), &mut pixmap.as_mut());
    pixmap.save_png("image.png").unwrap();
}

/*
use {
    ttf_parser::{Face, GlyphId},
    resvg::{tiny_skia::{Pixmap, Transform}, usvg::{fontdb::Database, Options, Tree}},
};

fn main() {
    let face = Face::parse(include_bytes!("NotoColorEmoji-Regular.ttf"), 0).unwrap();
    let code_point = char::from_u32(0x1F60A).unwrap();
    let glyph_id = face.glyph_index(code_point).unwrap();
    let svg_document = face.glyph_svg_image(glyph_id).unwrap();
    let opt = Options::default();
    let fontdb = Database::new();
    let tree = Tree::from_data(
        &svg_document.data,
        &opt,
        &fontdb,
    ).unwrap();
    let id = format!("glyph{}", glyph_id.0);
    let node = tree.node_by_id(&id).unwrap();
    let mut pixmap = Pixmap::new(1024, 1024).unwrap();
    resvg::render_node(&node, Transform::identity(), &mut pixmap.as_mut());
    pixmap.save_png("image.png").unwrap();
}
*/
