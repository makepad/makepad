//! Offline native encoder + sparse NAF oracle fixture. See trellis/tests.
#[cfg(feature = "mesh")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use makepad_ai_trellis::{
        backend::{gpu_download, gpu_upload},
        pixal_naf::PixalNaf,
        trellis::TrellisWeights,
        trellis_image::T2Image,
    };
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: pixal_naf_check DINO_NAF.safetensors OUTPUT.f32".into());
    }
    let weights = TrellisWeights::load(&args[1])?;
    let naf = PixalNaf::prepare(&weights)?;
    let image = T2Image {
        width: 32,
        height: 32,
        channels: 3,
        data: (0..3 * 32 * 32)
            .map(|i| ((i * 13) % 257) as f32 / 256.0)
            .collect(),
    };
    let values: Vec<f32> = (0..4 * 4 * 1024)
        .map(|i| ((i * 17) % 101) as f32 / 50.0 - 1.0)
        .collect();
    let values = gpu_upload(&values, 16, 1024)?;
    let mut uv = vec![[0.0, 0.0], [1.0, 1.0], [0.5, 0.5], [-1.0, 2.0]];
    uv.extend((0..29).map(|i| [i as f32 / 28.0, ((i * 7) % 29) as f32 / 28.0]));
    let encoded = naf.encode(&image)?;
    let mut bytes = Vec::new();
    for target in [16, 32] {
        let guide = naf.guide_from_features(&encoded, target, 4)?;
        let output = gpu_download(&guide.sample(&values, &uv)?)?;
        let direct = gpu_download(&naf.guide(&image, target, 4)?.sample(&values, &uv)?)?;
        let error = output
            .iter()
            .zip(&direct)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        if error > 1e-6 {
            return Err(format!("cached guide disagrees: {error}").into());
        }
        eprintln!("guide {target}: cached/direct max error {error}");
        for value in output {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(&args[2], bytes)?;
    Ok(())
}
#[cfg(not(feature = "mesh"))]
fn main() {
    eprintln!("build with --features mesh");
    std::process::exit(1);
}
