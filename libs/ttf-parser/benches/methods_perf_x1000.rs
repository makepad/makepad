use ttf_parser as ttf;

fn units_per_em(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.units_per_em());
        }
    })
}

fn width(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.width());
        }
    })
}

fn ascender(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.ascender());
        }
    })
}

fn underline_metrics(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.underline_metrics().unwrap());
        }
    })
}

fn strikeout_metrics(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.strikeout_metrics().unwrap());
        }
    })
}

fn subscript_metrics(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.subscript_metrics().unwrap());
        }
    })
}

fn x_height(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.x_height().unwrap());
        }
    })
}

fn glyph_hor_advance(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.glyph_hor_advance(ttf::GlyphId(2)).unwrap());
        }
    })
}

fn glyph_hor_side_bearing(bencher: &mut bencher::Bencher) {
    let font_data = std::fs::read("fonts/SourceSansPro-Regular.ttf").unwrap();
    let face = ttf::Face::parse(&font_data, 0).unwrap();
    bencher.iter(|| {
        for _ in 0..1000 {
            bencher::black_box(face.glyph_hor_side_bearing(ttf::GlyphId(2)).unwrap());
        }
    })
}

bencher::benchmark_group!(
    perf,
    units_per_em,
    width,
    ascender,
    underline_metrics,
    strikeout_metrics,
    subscript_metrics,
    x_height,
    glyph_hor_advance,
    glyph_hor_side_bearing
);
bencher::benchmark_main!(perf);
