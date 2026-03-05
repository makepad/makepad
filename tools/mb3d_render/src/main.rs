mod m3p;
mod formulas;
mod render;
mod lighting;
mod ssao;

use makepad_zune_png::PngEncoder;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let m3p_path = if args.len() > 1 {
        &args[1]
    } else {
        "local/mb3d/cathedral.m3p"
    };

    let output_path = if args.len() > 2 {
        &args[2]
    } else {
        "cathedral_test.png"
    };

    // Parse M3P
    eprintln!("Parsing M3P: {}", m3p_path);
    let m3p_file = match m3p::parse(m3p_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to parse M3P: {}", e);
            std::process::exit(1);
        }
    };

    // Print parsed info
    eprintln!("  Resolution: {}x{}", m3p_file.width, m3p_file.height);
    eprintln!("  Max iterations: {}, Min iterations: {}", m3p_file.iterations, m3p_file.min_iterations);
    eprintln!("  Zstart: {:.10e}, Zend: {:.10e}", m3p_file.z_start, m3p_file.z_end);
    eprintln!("  Camera: ({:.10e}, {:.10e}, {:.10e})", m3p_file.x_mid, m3p_file.y_mid, m3p_file.z_mid);
    eprintln!("  Zoom: {:.4e}", m3p_file.zoom);
    eprintln!("  StepWidth: {:.10e}", m3p_file.step_width);
    eprintln!("  FOV Y: {:.1}", m3p_file.fov_y);
    eprintln!("  RStop: {:.1}", m3p_file.rstop);
    eprintln!("  DEstop: {:.6}", m3p_file.de_stop);
    eprintln!("  ZStepDiv: {:.4}", m3p_file.z_step_div);
    eprintln!("  Julia: {} ({:.6}, {:.6}, {:.6})", m3p_file.is_julia, m3p_file.julia_x, m3p_file.julia_y, m3p_file.julia_z);
    eprintln!("  Hybrid type: {} (formulas: {})", m3p_file.addon.b_options1, m3p_file.addon.formula_count);

    for (i, f) in m3p_file.addon.formulas.iter().enumerate() {
        if f.iteration_count > 0 && i < m3p_file.addon.formula_count as usize {
            eprintln!("  Formula {}: #{} '{}' ({} iters, {} opts)",
                i, f.formula_nr, f.custom_name, f.iteration_count, f.option_count);
            for j in 0..f.option_count as usize {
                eprintln!("    opt[{}]: type={} value={:.6}", j, f.option_types[j], f.option_values[j]);
            }
        }
    }

    eprintln!("  View matrix:");
    for row in &m3p_file.view_matrix {
        eprintln!("    [{:.8}, {:.8}, {:.8}]", row[0], row[1], row[2]);
    }

    // Build formulas
    let formula_slots = formulas::build_formulas(&m3p_file);
    eprintln!("Built {} formula slots", formula_slots.len());

    if formula_slots.is_empty() {
        eprintln!("No formulas found, cannot render!");
        std::process::exit(1);
    }

    // Build render params
    let mut params = render::RenderParams::from_m3p(&m3p_file);

    // For testing, use smaller resolution
    let scale = std::env::var("SCALE").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
    if scale != 1.0 {
        params.camera.width = (m3p_file.width as f64 * scale) as i32;
        params.camera.height = (m3p_file.height as f64 * scale) as i32;
        eprintln!("Rendering at {}x{} (scale={:.2})", params.camera.width, params.camera.height, scale);
    }

    // Debug: evaluate DE at center pixel along the ray
    {
        let (origin, dir) = params.camera.ray_for_pixel(params.camera.width / 2, params.camera.height / 2);
        eprintln!("\n  DEBUG center pixel:");
        eprintln!("    origin: ({:.10e}, {:.10e}, {:.10e})", origin.x, origin.y, origin.z);
        eprintln!("    dir:    ({:.6e}, {:.6e}, {:.6e})", dir.x, dir.y, dir.z);

        // Trace ray march steps using the MB3D step formula
        let mut t = 0.0f64;
        let de_stop = params.de_stop;
        let de_floor = params.de_floor;
        for step_i in 0..20 {
            let pos = origin.add(dir.scale(t));
            let (iters, raw_de) = formulas::hybrid_de(
                (pos.x, pos.y, pos.z), &formula_slots, &params.iter_params
            );
            let de = raw_de.max(de_floor);
            let hit = iters >= params.iter_params.max_iters || de < de_stop;
            eprintln!("    step {}: t={:.4e} de={:.4e} destop={:.4e} iters={} hit={} step={:.4e}",
                step_i, t, de, de_stop, iters, hit, de * params.s_z_step_div);
            if hit { break; }
            let step = de * params.s_z_step_div;
            t += step;
            if t > params.max_ray_length { eprintln!("    (exceeded max_ray_length)"); break; }
        }
        eprintln!();
    }

    // Render
        let pixels = render::render(&formula_slots, &params, &m3p_file.lighting, &m3p_file.ssao);

    // Encode PNG
    let w = params.camera.width as usize;
    let h = params.camera.height as usize;
    eprintln!("Encoding PNG {}x{} ...", w, h);

    let options = EncoderOptions::default()
        .set_width(w)
        .set_height(h)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);

    let mut encoder = PngEncoder::new(&pixels, options);
    let mut out = Vec::new();
    match encoder.encode(&mut out) {
        Ok(_) => {},
        Err(e) => {
            eprintln!("PNG encode failed: {:?}", e);
            std::process::exit(1);
        }
    }

    std::fs::write(output_path, &out).expect("Failed to write output file");
    eprintln!("Wrote {} ({} bytes)", output_path, out.len());
}
