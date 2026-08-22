//! Full HunyuanPaintPipeline job: unit-cube atlas + solid reference.
//! Default 128² / 6 views. Not a service claim.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("PBR_PIPE_CANARY_FAIL {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_PIPE_CANARY_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_ai_paint::hunyuan::{acknowledge_license, LICENSE_TEXT_SHA256};
    use makepad_ai_paint::mesh::TriMesh;
    use makepad_ai_paint::native_exec::NativeHunyuanExec;
    use makepad_ai_paint::pipeline::{
        HunyuanPaintPipeline, MemoryProfile, PaintConfig, PaintInputs,
    };
    use std::time::Instant;

    let size = std::env::var("MAKEPAD_PBR_PIPE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(128u32);
    if size == 0 || size % 8 != 0 {
        return Err(format!("MAKEPAD_PBR_PIPE_SIZE={size} must be a positive multiple of 8"));
    }
    let tex = size.max(128);
    let mesh = TriMesh::unit_cube_atlas();
    let ref_w = 64u32;
    let ref_h = 64u32;
    let mut reference_rgb = vec![0u8; (ref_w * ref_h * 3) as usize];
    for y in 0..ref_h {
        for x in 0..ref_w {
            let i = ((y * ref_w + x) * 3) as usize;
            reference_rgb[i] = (x * 4) as u8;
            reference_rgb[i + 1] = (y * 4) as u8;
            reference_rgb[i + 2] = 160;
        }
    }
    let exec = NativeHunyuanExec::discover().map_err(|e| e.to_string())?;
    let config = PaintConfig {
        num_views_max: 6,
        resolution: size,
        texture_size: tex,
        view_select_res: size.max(128),
        depth_size: size.max(256),
        ortho_scale: 1.2,
        profile: MemoryProfile::Standard24g,
        seed: 0,
    };
    let license = acknowledge_license(true, LICENSE_TEXT_SHA256).map_err(|e| e.to_string())?;
    let mut pipe = HunyuanPaintPipeline::new(exec, config).with_license_acknowledgement(license);
    let inputs = PaintInputs {
        mesh: &mesh,
        reference_rgb: &reference_rgb,
        ref_width: ref_w,
        ref_height: ref_h,
        mesh_sha256: None,
        reference_sha256: None,
        baked_ao: None,
    };
    let t0 = Instant::now();
    let mut denoise_t0 = None;
    let mut denoise_s = 0.0f64;
    let set = pipe
        .generate(&inputs, &mut |p| {
            let stage = format!("{:?}", p.stage);
            if stage.contains("Denoise") {
                if denoise_t0.is_none() {
                    denoise_t0 = Some(Instant::now());
                }
                if p.current == p.total {
                    if let Some(t) = denoise_t0 {
                        denoise_s = t.elapsed().as_secs_f64();
                    }
                }
            }
            println!(
                "PBR_PIPE_STAGE {:?} {}/{}",
                p.stage, p.current, p.total
            );
            true
        })
        .map_err(|e| e.to_string())?;
    println!("PBR_PIPE_JOB_S {:.3} tex={}", t0.elapsed().as_secs_f64(), tex);
    println!("PBR_PIPE_DENOISE_S {:.3}", denoise_s);
    let albedo = set
        .albedo
        .map
        .as_ref()
        .ok_or("pipeline missing albedo")?;
    let want = albedo
        .expected_len()
        .map_err(|e| format!("albedo layout: {e}"))?;
    if albedo.data.len() != want {
        return Err(format!(
            "albedo bytes {} want {want} ({}x{})",
            albedo.data.len(),
            albedo.width,
            albedo.height
        ));
    }
    println!(
        "PBR_PIPE_CANARY_OK albedo={}x{} orm={}",
        albedo.width,
        albedo.height,
        set.packed_orm.is_some()
    );
    if let Ok(dir) = std::env::var("MAKEPAD_PBR_DUMP_DIR") {
        dump_pipeline_pngs(&dir, &reference_rgb, ref_w, ref_h, &set)?;
        println!("PBR_PIPE_DUMP {dir}");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn dump_pipeline_pngs(
    dir: &str,
    reference_rgb: &[u8],
    ref_w: u32,
    ref_h: u32,
    set: &makepad_ai_paint::contract::PbrMaterialSet,
) -> Result<(), String> {
    use makepad_ai_paint::png::{encode_png, PngColor};
    std::fs::create_dir_all(dir).map_err(|e| format!("dump dir {dir}: {e}"))?;
    let write = |name: &str, w: u32, h: u32, color: PngColor, data: &[u8]| -> Result<(), String> {
        let path = format!("{dir}/{name}");
        let png = encode_png(w, h, color, data);
        std::fs::write(&path, png).map_err(|e| format!("write {path}: {e}"))?;
        Ok(())
    };
    write("reference.png", ref_w, ref_h, PngColor::Rgb, reference_rgb)?;
    if let Some(m) = set.albedo.map.as_ref() {
        write("albedo.png", m.width, m.height, PngColor::Rgb, &m.data)?;
    }
    if let Some(m) = set.normal.map.as_ref() {
        write("normal.png", m.width, m.height, PngColor::Rgb, &m.data)?;
    }
    if let Some(m) = set.roughness.map.as_ref() {
        write("roughness.png", m.width, m.height, PngColor::Gray, &m.data)?;
    }
    if let Some(m) = set.metallic.map.as_ref() {
        write("metallic.png", m.width, m.height, PngColor::Gray, &m.data)?;
    }
    if let Some(m) = set.packed_orm.as_ref() {
        write("orm.png", m.width, m.height, PngColor::Rgb, &m.data)?;
    }
    Ok(())
}
