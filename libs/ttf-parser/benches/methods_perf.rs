use ttf_parser as ttf;

fn from_data_ttf(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    bencher.iter(|| {
        bencher::black_box(ttf::Face::parse(&font_data, 0).unwrap());
    })
}

fn from_data_otf_cff(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.otf").unwrap();
    bencher.iter(|| {
        bencher::black_box(ttf::Face::parse(&font_data, 0).unwrap());
    })
}

fn from_data_otf_cff2(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansVariable-Roman.otf").unwrap();
    bencher.iter(|| {
        bencher::black_box(ttf::Face::parse(&font_data, 0).unwrap());
    })
}

fn outline_glyph_8_from_glyf(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(8), &mut Builder(0)))
}

fn outline_glyph_276_from_glyf(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    let mut b = Builder(0);
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(276), &mut b))
}

fn outline_glyph_8_from_cff(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(8), &mut Builder(0)))
}

fn outline_glyph_276_from_cff(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(276), &mut Builder(0)))
}

fn outline_glyph_8_from_cff2(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansVariable-Roman.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(8), &mut Builder(0)))
}

fn outline_glyph_276_from_cff2(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansVariable-Roman.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| face.outline_glyph(ttf::GlyphId(276), &mut Builder(0)))
}

fn family_name(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        bencher::black_box(
            face.names()
                .into_iter()
                .find(|name| name.name_id == ttf::name_id::FULL_NAME)
                .and_then(|name| name.to_string()),
        );
    })
}

fn glyph_name_post_8(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    assert_eq!(face.glyph_name(ttf::GlyphId(8)).unwrap(), "G");
    bencher.iter(|| {
        bencher::black_box(face.glyph_name(ttf::GlyphId(8)).unwrap());
    })
}

fn glyph_name_post_276(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    assert_eq!(face.glyph_name(ttf::GlyphId(276)).unwrap(), "uni1EAB");
    bencher.iter(|| {
        bencher::black_box(face.glyph_name(ttf::GlyphId(276)).unwrap());
    })
}

fn glyph_name_cff_8(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    assert_eq!(face.glyph_name(ttf::GlyphId(8)).unwrap(), "G");
    bencher.iter(|| {
        bencher::black_box(face.glyph_name(ttf::GlyphId(8)).unwrap());
    })
}

fn glyph_name_cff_276(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.otf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    assert_eq!(face.glyph_name(ttf::GlyphId(276)).unwrap(), "uni1EAB");
    bencher.iter(|| {
        bencher::black_box(face.glyph_name(ttf::GlyphId(276)).unwrap());
    })
}

fn glyph_index_u41(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        bencher::black_box(face.glyph_index('A').unwrap());
    })
}

struct Builder(usize);

impl ttf_parser::OutlineBuilder for Builder {
    #[inline]
    fn move_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }

    #[inline]
    fn line_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }

    #[inline]
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 2;
    }

    #[inline]
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 3;
    }

    #[inline]
    fn close(&mut self) {
        self.0 += 1;
    }
}

bencher::benchmark_group!(
    perf,
    from_data_ttf,
    from_data_otf_cff,
    from_data_otf_cff2,
    outline_glyph_8_from_glyf,
    outline_glyph_276_from_glyf,
    outline_glyph_8_from_cff,
    outline_glyph_276_from_cff,
    outline_glyph_8_from_cff2,
    outline_glyph_276_from_cff2,
    glyph_name_post_8,
    glyph_name_post_276,
    glyph_name_cff_8,
    glyph_name_cff_276,
    family_name,
    glyph_index_u41
);
bencher::benchmark_main!(perf);
