use {makepad_draw::*, makepad_draw::icon_atlas::PathCommand, std::str, ttf_parser::Face, tiny_skia::*};

fn main() {
    let face = Face::parse(include_bytes!("NotoColorEmoji-Regular.ttf"), 0).unwrap();
    let id = face.glyph_index(char::from_u32(0x1F38E).unwrap()).unwrap();
    let document = face.glyph_svg_image(id).unwrap();
    let body = str::from_utf8(document.data).unwrap();
    let mut errors = Some(Vec::new());
    let doc = makepad_html::parse_html(body, &mut errors, InternLiveId::No);

    let mut pixmap = Pixmap::new(256, 256).unwrap();
    let mut walker = doc.new_walker();
    while !walker.done() {
        if let Some(tag) = walker.open_tag_lc() {
            match tag {
                live_id!(path) => {
                    let d = walker.find_attr_lc(live_id!(d)).unwrap();
                    let mut builder = PathBuilder::new();
                    for command in icon_atlas::parse_svg_path(d.as_bytes()).unwrap() {
                        match command {
                            PathCommand::MoveTo(p1) => {
                                builder.move_to(p1.x as f32, p1.y as f32);
                            }
                            PathCommand::LineTo(p1) => {
                                builder.move_to(p1.x as f32, p1.y as f32);
                            }
                            PathCommand::ArcTo(_, _, _, _, _) => {
                                unimplemented!()
                            }
                            PathCommand::QuadraticTo(p1, p2) => {
                                builder.quad_to(p1.x as f32, p1.y as f32, p2.x as f32, p2.y as f32);
                            }
                            PathCommand::CubicTo(p1, p2, p3) => {
                                builder.cubic_to(p1.x as f32, p1.y as f32, p2.x as f32, p2.y as f32, p3.x as f32, p3.y as f32);
                            }
                            PathCommand::Close => {
                                builder.close();
                            }
                        }
                    }
                    let path = builder.finish().unwrap();

                    if let Some(transform) = walker.find_attr_lc(live_id!(transform)) {
                        println!("transform {:?}", transform);
                    }

                    if let Some(fill) = walker.find_attr_lc(live_id!(fill)) {
                        let mut paint = Paint::default();
                        if let Some('#') = fill.chars().next() {
                            let color = makepad_live_tokenizer::colorhex::hex_bytes_to_u32(fill[1..].as_bytes()).unwrap();
                            let r = (color >> 24 & 0xFF) as u8;
                            let g = (color >> 16 & 0xFF) as u8;
                            let b = (color >> 8 & 0xFF) as u8;
                            let a = (color >> 0 & 0xFF) as u8;
                            paint.set_color_rgba8(r, g, b, a);
                        }
                        pixmap.fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
                _ => {}
            }
        }
        walker.walk();
    }
    pixmap.save_png("image.png").unwrap();
}
