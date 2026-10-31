// Real-world benchmark workloads for Relax (phase 1): the same source under rustc --release and
// `makepad run`. Each workload is `setup() -> iteration closure`; the timing loop, output line and env
// are in harness.rs (shared by path with the commercial set, apps/commercial/relax_tests).
// Usage: relax-rwbench <workload>...
mod harness;
mod audio;
mod box3d;
mod box3d_scene;
mod csg;
mod inputs;
mod layout;
mod llm;
mod maps;
mod parsers;
mod script;
mod sdf;
mod text;

fn main() {
    let all: [(&str, fn() -> Box<dyn FnMut() -> u64>); 18] = [
        ("text_shape_raster", text::setup),
        ("sdf_glyphs", sdf::setup),
        ("inflate_real", inputs::inflate_setup),
        ("zip_read", inputs::zip_setup),
        ("png_decode", inputs::png_setup),
        ("brotli_decode", inputs::brotli_setup),
        ("maps_tile_decode", maps::setup),
        ("layout_widgets", layout::setup),
        ("llm_host", llm::setup),
        ("rust_tokenize", parsers::rust_tokens_setup),
        ("markdown_parse", parsers::markdown_setup),
        ("html_parse", parsers::html_setup),
        ("piano_render", audio::piano_setup),
        ("ogg_decode", audio::ogg_setup),
        ("audio_mix", audio::mix_setup),
        ("splash_eval", script::setup),
        ("csg_boolean", csg::setup),
        ("box3d_physics", box3d::setup),
    ];
    harness::main_with(&all);
}

pub fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
