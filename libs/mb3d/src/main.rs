mod m3p;
mod formulas;
mod render;
mod lighting;

use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;

struct Options {
    m3p_path: String,
    output_path: String,
    scale: f64,
}

fn usage(program: &str) -> String {
    format!("Usage: {program} [--scale <factor>] [input.m3p] [output.png]")
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "makepad-mb3d-render".to_string());
    let mut scale = 1.0f64;
    let mut positional = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scale" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for --scale\n{}", usage(&program)))?;
                scale = value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid --scale value '{value}'"))?;
                if !scale.is_finite() || scale <= 0.0 {
                    return Err(format!("--scale must be a positive finite number\n{}", usage(&program)));
                }
            }
            "-h" | "--help" => {
                println!("{}", usage(&program));
                return Ok(None);
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() > 2 {
        return Err(usage(&program));
    }

    Ok(Some(Options {
        m3p_path: positional
            .first()
            .cloned()
            .unwrap_or_else(|| "local/mb3d/cathedral.m3p".to_string()),
        output_path: positional
            .get(1)
            .cloned()
            .unwrap_or_else(|| "cathedral_test.png".to_string()),
        scale,
    }))
}

fn main() {
    let Some(options) = (match parse_args() {
        Ok(options) => options,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }) else {
        return;
    };

    eprintln!("Parsing M3P: {}", options.m3p_path);
    let m3p_file = match m3p::parse(&options.m3p_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to parse M3P: {}", e);
            std::process::exit(1);
        }
    };

    let formula_slots = formulas::build_formulas(&m3p_file);
    if formula_slots.is_empty() {
        eprintln!("No formulas found, cannot render!");
        std::process::exit(1);
    }

    let mut params = render::RenderParams::from_m3p(&m3p_file);
    if (options.scale - 1.0).abs() > f64::EPSILON {
        params.camera.width = (m3p_file.width as f64 * options.scale).round().max(1.0) as i32;
        params.camera.height = (m3p_file.height as f64 * options.scale).round().max(1.0) as i32;
    }

    let pixels = render::render(&formula_slots, &params, &m3p_file.lighting, &m3p_file.ssao);

    let w = params.camera.width as usize;
    let h = params.camera.height as usize;
    eprintln!("Encoding PNG {}x{} ...", w, h);

    let encoder_options = EncoderOptions::default()
        .set_width(w)
        .set_height(h)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);

    let mut encoder = PngEncoder::new(&pixels, encoder_options);
    let mut out = Vec::new();
    match encoder.encode(&mut out) {
        Ok(_) => {},
        Err(e) => {
            eprintln!("PNG encode failed: {:?}", e);
            std::process::exit(1);
        }
    }

    std::fs::write(&options.output_path, &out).expect("Failed to write output file");
    eprintln!("Wrote {} ({} bytes)", options.output_path, out.len());
}
