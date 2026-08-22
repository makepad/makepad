//! Native UNet conv_in + first ResNet vs official Hunyuan dump.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("PBR_UNET_CANARY_FAIL {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_UNET_CANARY_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_ai_paint::dino_proj::DinoProj;
    use makepad_ai_paint::unet_extras::{ExtraFlags, ExtraInputs};
    use makepad_ai_paint::unet_first::{arange_nchw_planar, UnetFirst};
    use std::path::PathBuf;
    use std::time::Instant;

    std::env::set_var("MAKEPAD_PBR_TAP_PARITY", "1");
    let weights = std::env::var("MAKEPAD_HUNYUAN_UNET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(
                r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1\unet\diffusion_pytorch_model.bin",
            )
        });
    let t0 = Instant::now();
    let unet = UnetFirst::load(&weights)?;
    println!("PBR_UNET_LOAD_S {:.3}", t0.elapsed().as_secs_f64());
    let h = 8usize;
    // Official dump is NCHW arange; convert to planar [c][y*x]
    let nchw = arange_nchw_planar(12, h, h);
    let mut planar = vec![0.0f32; 12 * h * h];
    for c in 0..12 {
        for y in 0..h {
            for x in 0..h {
                let n = ((c * h + y) * h + x) as usize;
                planar[c * h * h + y * h + x] = nchw[n];
            }
        }
    }
    // wait: arange_nchw_planar already fills in NCHW-flat order if we interpret
    // index i as nchw. Same as official torch.arange(...).reshape(1,12,h,h)
    // Official x12 is cat([noise, normal, position], 1) each 4ch from same arange
    // of 4*h*h. Rebuild that, not a 12-ch arange.
    let n4 = 4 * h * h;
    let noise: Vec<f32> = (0..n4).map(|i| i as f32 / n4 as f32).collect();
    let mut x12 = Vec::with_capacity(12 * h * h);
    // planar: 4 noise + 4 normal + 4 position, each NCHW->planar
    for src in [
        noise.as_slice(),
        &noise.iter().map(|v| v * 0.25 + 0.1).collect::<Vec<_>>(),
        &noise.iter().map(|v| v * 0.5 - 0.2).collect::<Vec<_>>(),
    ] {
        // src is NCHW 4xhxh flat
        for c in 0..4 {
            for y in 0..h {
                for x in 0..h {
                    x12.push(src[(c * h + y) * h + x]);
                }
            }
        }
    }
    let t0 = Instant::now();
    let conv = unet.conv_in(&x12, h, h)?;
    println!(
        "PBR_UNET_CONV_IN_S {:.4} digest={} head={:?}",
        t0.elapsed().as_secs_f64(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&conv),
        &conv[..8]
    );
    let temb = unet.timestep_embedding(999.0)?;
    println!(
        "PBR_UNET_TEMB digest={}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&temb)
    );
    let t0 = Instant::now();
    let res0 = unet.resnet0(&conv, h, h, &temb)?;
    println!(
        "PBR_UNET_RESNET0_S {:.4} digest={} head={:?}",
        t0.elapsed().as_secs_f64(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&res0),
        &res0[..8]
    );
    let t0 = Instant::now();
    let res1 = unet.resnet(
        &res0,
        h,
        h,
        &temb,
        "unet.down_blocks.0.resnets.1",
        320,
    )?;
    println!(
        "PBR_UNET_RESNET1_S {:.4} digest={} head={:?}",
        t0.elapsed().as_secs_f64(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&res1),
        &res1[..8]
    );
    let t0 = Instant::now();
    let (down, dw, dh) = unet.downsample(
        &res1,
        h,
        h,
        "unet.down_blocks.0.downsamplers.0.conv",
        320,
    )?;
    println!(
        "PBR_UNET_DOWN_S {:.4} shape=320x{dw}x{dh} digest={} head={:?}",
        t0.elapsed().as_secs_f64(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&down),
        &down[..8]
    );
    let encoder = unet.learned_text_clip_albedo()?;
    let t0 = Instant::now();
    let attn0 = unet.transformer2d(
        &res0,
        h,
        h,
        &encoder,
        77,
        "unet.down_blocks.0.attentions.0",
        320,
        5,
    )?;
    println!(
        "PBR_UNET_ATTN0_S {:.4} digest={} head={:?}",
        t0.elapsed().as_secs_f64(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&attn0),
        &attn0[..8]
    );
    let attn1 = unet.transformer2d(
        &res1,
        h,
        h,
        &encoder,
        77,
        "unet.down_blocks.0.attentions.1",
        320,
        5,
    )?;
    println!(
        "PBR_UNET_ATTN1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&attn1),
        &attn1[..8]
    );
    let d1_res0 = unet.resnet(
        &down,
        dw,
        dh,
        &temb,
        "unet.down_blocks.1.resnets.0",
        320,
    )?;
    println!(
        "PBR_UNET_D1_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d1_res0),
        &d1_res0[..8]
    );
    let d1_res1 = unet.resnet(
        &d1_res0,
        dw,
        dh,
        &temb,
        "unet.down_blocks.1.resnets.1",
        640,
    )?;
    println!(
        "PBR_UNET_D1_RES1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d1_res1),
        &d1_res1[..8]
    );
    let d1_attn0 = unet.transformer2d(
        &d1_res0,
        dw,
        dh,
        &encoder,
        77,
        "unet.down_blocks.1.attentions.0",
        640,
        10,
    )?;
    println!(
        "PBR_UNET_D1_ATTN0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d1_attn0),
        &d1_attn0[..8]
    );
    let d1_attn1 = unet.transformer2d(
        &d1_res1,
        dw,
        dh,
        &encoder,
        77,
        "unet.down_blocks.1.attentions.1",
        640,
        10,
    )?;
    println!(
        "PBR_UNET_D1_ATTN1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d1_attn1),
        &d1_attn1[..8]
    );
    let (d1_down, d1w, d1h) = unet.downsample(
        &d1_res1,
        dw,
        dh,
        "unet.down_blocks.1.downsamplers.0.conv",
        640,
    )?;
    println!(
        "PBR_UNET_D1_DOWN shape=640x{d1w}x{d1h} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d1_down),
        &d1_down[..8]
    );
    let d2_res0 = unet.resnet(
        &d1_down,
        d1w,
        d1h,
        &temb,
        "unet.down_blocks.2.resnets.0",
        640,
    )?;
    println!(
        "PBR_UNET_D2_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d2_res0),
        &d2_res0[..8]
    );
    let d2_res1 = unet.resnet(
        &d2_res0,
        d1w,
        d1h,
        &temb,
        "unet.down_blocks.2.resnets.1",
        1280,
    )?;
    println!(
        "PBR_UNET_D2_RES1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d2_res1),
        &d2_res1[..8]
    );
    let d2_attn0 = unet.transformer2d(
        &d2_res0,
        d1w,
        d1h,
        &encoder,
        77,
        "unet.down_blocks.2.attentions.0",
        1280,
        20,
    )?;
    println!(
        "PBR_UNET_D2_ATTN0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d2_attn0),
        &d2_attn0[..8]
    );
    let (d2_down, d2w, d2h) = unet.downsample(
        &d2_res1,
        d1w,
        d1h,
        "unet.down_blocks.2.downsamplers.0.conv",
        1280,
    )?;
    println!(
        "PBR_UNET_D2_DOWN shape=1280x{d2w}x{d2h} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d2_down),
        &d2_down[..8]
    );
    let d3_res0 = unet.resnet(
        &d2_down,
        d2w,
        d2h,
        &temb,
        "unet.down_blocks.3.resnets.0",
        1280,
    )?;
    println!(
        "PBR_UNET_D3_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d3_res0),
        &d3_res0[..8]
    );
    let d3_res1 = unet.resnet(
        &d3_res0,
        d2w,
        d2h,
        &temb,
        "unet.down_blocks.3.resnets.1",
        1280,
    )?;
    println!(
        "PBR_UNET_D3_RES1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&d3_res1),
        &d3_res1[..8]
    );
    let mid_res0 = unet.resnet(&d3_res1, d2w, d2h, &temb, "unet.mid_block.resnets.0", 1280)?;
    println!(
        "PBR_UNET_MID_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&mid_res0),
        &mid_res0[..8]
    );
    let mid_attn = unet.transformer2d(
        &mid_res0,
        d2w,
        d2h,
        &encoder,
        77,
        "unet.mid_block.attentions.0",
        1280,
        20,
    )?;
    println!(
        "PBR_UNET_MID_ATTN digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&mid_attn),
        &mid_attn[..8]
    );
    let mid_res1 = unet.resnet(&mid_attn, d2w, d2h, &temb, "unet.mid_block.resnets.1", 1280)?;
    println!(
        "PBR_UNET_MID_RES1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&mid_res1),
        &mid_res1[..8]
    );
    let enc_mr = unet.learned_text_clip_mr()?;
    let mr_in: Vec<f32> = res0.iter().map(|v| v * 0.7 + 0.05).collect();
    let ref_in: Vec<f32> = res0.iter().map(|v| v * 0.5 + 0.1).collect();
    let dino_raw: Vec<f32> = (0..1536).map(|i| i as f32 / 1536.0).collect();
    let dino_tok = DinoProj::load_from_unet_bin(&weights)?.forward(&dino_raw, 1)?;
    println!(
        "PBR_UNET_DINO_PROJ digest={}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&dino_tok)
    );
    let prefix0 = "unet.down_blocks.0.attentions.0";
    let mda = unet.transformer2d_extras(ExtraInputs {
        samples: &[&res0, &mr_in],
        samples_gpu: None,
        encoders: &[&encoder, &enc_mr],
        width: h,
        height: h,
        channels: 320,
        heads: 5,
        prefix: prefix0,
        flags: ExtraFlags {
            mda: true,
            ..ExtraFlags::default()
        },
        dino: None,
        ref_samples: None,
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 1,
        voxel_xyz: None,
        voxel_res: 64,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })?;
    let mda_cat = UnetFirst::concat_planar(&mda[0], &mda[1]);
    println!(
        "PBR_UNET_MDA digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&mda_cat),
        &mda_cat[..8]
    );
    let dino_only = unet.transformer2d_extras(ExtraInputs {
        samples: &[&res0, &mr_in],
        samples_gpu: None,
        encoders: &[&encoder, &enc_mr],
        width: h,
        height: h,
        channels: 320,
        heads: 5,
        prefix: prefix0,
        flags: ExtraFlags {
            dino: true,
            ..ExtraFlags::default()
        },
        dino: Some(&dino_tok),
        ref_samples: None,
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 1,
        voxel_xyz: None,
        voxel_res: 64,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })?;
    let dino_cat = UnetFirst::concat_planar(&dino_only[0], &dino_only[1]);
    println!(
        "PBR_UNET_DINO digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&dino_cat),
        &dino_cat[..8]
    );
    let mda_dino = unet.transformer2d_extras(ExtraInputs {
        samples: &[&res0, &mr_in],
        samples_gpu: None,
        encoders: &[&encoder, &enc_mr],
        width: h,
        height: h,
        channels: 320,
        heads: 5,
        prefix: prefix0,
        flags: ExtraFlags {
            mda: true,
            dino: true,
            ..ExtraFlags::default()
        },
        dino: Some(&dino_tok),
        ref_samples: None,
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 1,
        voxel_xyz: None,
        voxel_res: 64,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })?;
    let mda_dino_cat = UnetFirst::concat_planar(&mda_dino[0], &mda_dino[1]);
    println!(
        "PBR_UNET_MDA_DINO digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&mda_dino_cat),
        &mda_dino_cat[..8]
    );
    let refs: [&[f32]; 1] = [&ref_in];
    let ra = unet.transformer2d_extras(ExtraInputs {
        samples: &[&res0, &mr_in],
        samples_gpu: None,
        encoders: &[&encoder, &enc_mr],
        width: h,
        height: h,
        channels: 320,
        heads: 5,
        prefix: prefix0,
        flags: ExtraFlags {
            ra: true,
            ..ExtraFlags::default()
        },
        dino: None,
        ref_samples: Some(&refs),
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 1,
        voxel_xyz: None,
        voxel_res: 64,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })?;
    let ra_cat = UnetFirst::concat_planar(&ra[0], &ra[1]);
    println!(
        "PBR_UNET_REF digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&ra_cat),
        &ra_cat[..8]
    );
    let v2: Vec<f32> = res0.iter().map(|v| v * 0.8 + 0.02).collect();
    let mr_v2: Vec<f32> = mr_in.iter().map(|v| v * 0.8 + 0.02).collect();
    let mut voxel_xyz = Vec::new();
    if let Ok(acts_path) = std::env::var("PBR_UNET_ACTS") {
        if let Ok(acts) = load_acts(&acts_path) {
            if let Ok(flat) = need(&acts, "ma_voxel") {
                for c in flat.chunks_exact(3) {
                    voxel_xyz.push([c[0] as u32, c[1] as u32, c[2] as u32]);
                }
            }
        }
    }
    let ma_out = if voxel_xyz.len() == 2 * h * h {
        let ma = unet.transformer2d_extras(ExtraInputs {
            samples: &[&res0, &v2, &mr_in, &mr_v2],
            samples_gpu: None,
            encoders: &[&encoder, &encoder, &enc_mr, &enc_mr],
            width: h,
            height: h,
            channels: 320,
            heads: 5,
            prefix: prefix0,
            flags: ExtraFlags {
                ma: true,
                ..ExtraFlags::default()
            },
            dino: None,
            ref_samples: None,
        ref_tokens: None,
            ref_scale: 1.0,
            n_views: 2,
            voxel_xyz: Some(&voxel_xyz),
            voxel_res: 64,
            mva_scale: 1.0,
            dinos: None,
            ref_scales: None,
        })?;
        let cat = [&ma[0], &ma[1], &ma[2], &ma[3]]
            .iter()
            .fold(Vec::new(), |mut a, s| {
                a.extend_from_slice(s);
                a
            });
        println!(
            "PBR_UNET_MA digest={} head={:?}",
            makepad_ai_paint::numerical_fixtures::digest_f32(&cat),
            &cat[..8]
        );
        Some(cat)
    } else {
        println!("PBR_UNET_MA skipped (no voxel acts)");
        None
    };
    let up_cat0 = UnetFirst::concat_planar(&mid_res1, &d3_res1);
    let up0_r0 = unet.resnet(
        &up_cat0,
        d2w,
        d2h,
        &temb,
        "unet.up_blocks.0.resnets.0",
        2560,
    )?;
    println!(
        "PBR_UNET_UP0_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up0_r0),
        &up0_r0[..8]
    );
    let up_cat1 = UnetFirst::concat_planar(&up0_r0, &d3_res0);
    let up0_r1 = unet.resnet(
        &up_cat1,
        d2w,
        d2h,
        &temb,
        "unet.up_blocks.0.resnets.1",
        2560,
    )?;
    println!(
        "PBR_UNET_UP0_RES1 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up0_r1),
        &up0_r1[..8]
    );
    let up_cat2 = UnetFirst::concat_planar(&up0_r1, &d2_down);
    let up0_r2 = unet.resnet(
        &up_cat2,
        d2w,
        d2h,
        &temb,
        "unet.up_blocks.0.resnets.2",
        2560,
    )?;
    println!(
        "PBR_UNET_UP0_RES2 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up0_r2),
        &up0_r2[..8]
    );
    let (up0_up, upw, uph) = unet.upsample(
        &up0_r2,
        d2w,
        d2h,
        "unet.up_blocks.0.upsamplers.0.conv",
        1280,
    )?;
    println!(
        "PBR_UNET_UP0_UP shape=1280x{upw}x{uph} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up0_up),
        &up0_up[..8]
    );
    let up1_r0 = unet.resnet(
        &UnetFirst::concat_planar(&up0_up, &d2_res1),
        upw,
        uph,
        &temb,
        "unet.up_blocks.1.resnets.0",
        2560,
    )?;
    println!(
        "PBR_UNET_UP1_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up1_r0),
        &up1_r0[..8]
    );
    let up1_a0 = unet.transformer2d(
        &up1_r0,
        upw,
        uph,
        &encoder,
        77,
        "unet.up_blocks.1.attentions.0",
        1280,
        20,
    )?;
    println!(
        "PBR_UNET_UP1_ATTN0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up1_a0),
        &up1_a0[..8]
    );
    let up1_r1 = unet.resnet(
        &UnetFirst::concat_planar(&up1_a0, &d2_res0),
        upw,
        uph,
        &temb,
        "unet.up_blocks.1.resnets.1",
        2560,
    )?;
    let up1_a1 = unet.transformer2d(
        &up1_r1,
        upw,
        uph,
        &encoder,
        77,
        "unet.up_blocks.1.attentions.1",
        1280,
        20,
    )?;
    let up1_r2 = unet.resnet(
        &UnetFirst::concat_planar(&up1_a1, &d1_down),
        upw,
        uph,
        &temb,
        "unet.up_blocks.1.resnets.2",
        1920,
    )?;
    let up1_a2 = unet.transformer2d(
        &up1_r2,
        upw,
        uph,
        &encoder,
        77,
        "unet.up_blocks.1.attentions.2",
        1280,
        20,
    )?;
    let (up1_up, u1w, u1h) = unet.upsample(
        &up1_a2,
        upw,
        uph,
        "unet.up_blocks.1.upsamplers.0.conv",
        1280,
    )?;
    println!(
        "PBR_UNET_UP1_UP shape=1280x{u1w}x{u1h} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up1_up),
        &up1_up[..8]
    );
    let up2_r0 = unet.resnet(
        &UnetFirst::concat_planar(&up1_up, &d1_res1),
        u1w,
        u1h,
        &temb,
        "unet.up_blocks.2.resnets.0",
        1920,
    )?;
    println!(
        "PBR_UNET_UP2_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up2_r0),
        &up2_r0[..8]
    );
    let up2_a0 = unet.transformer2d(
        &up2_r0,
        u1w,
        u1h,
        &encoder,
        77,
        "unet.up_blocks.2.attentions.0",
        640,
        10,
    )?;
    let up2_r1 = unet.resnet(
        &UnetFirst::concat_planar(&up2_a0, &d1_res0),
        u1w,
        u1h,
        &temb,
        "unet.up_blocks.2.resnets.1",
        1280,
    )?;
    let up2_a1 = unet.transformer2d(
        &up2_r1,
        u1w,
        u1h,
        &encoder,
        77,
        "unet.up_blocks.2.attentions.1",
        640,
        10,
    )?;
    let up2_r2 = unet.resnet(
        &UnetFirst::concat_planar(&up2_a1, &down),
        u1w,
        u1h,
        &temb,
        "unet.up_blocks.2.resnets.2",
        960,
    )?;
    let up2_a2 = unet.transformer2d(
        &up2_r2,
        u1w,
        u1h,
        &encoder,
        77,
        "unet.up_blocks.2.attentions.2",
        640,
        10,
    )?;
    let (up2_up, u2w, u2h) = unet.upsample(
        &up2_a2,
        u1w,
        u1h,
        "unet.up_blocks.2.upsamplers.0.conv",
        640,
    )?;
    println!(
        "PBR_UNET_UP2_UP shape=640x{u2w}x{u2h} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up2_up),
        &up2_up[..8]
    );
    let up3_r0 = unet.resnet(
        &UnetFirst::concat_planar(&up2_up, &res1),
        u2w,
        u2h,
        &temb,
        "unet.up_blocks.3.resnets.0",
        960,
    )?;
    println!(
        "PBR_UNET_UP3_RES0 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up3_r0),
        &up3_r0[..8]
    );
    let up3_a0 = unet.transformer2d(
        &up3_r0,
        u2w,
        u2h,
        &encoder,
        77,
        "unet.up_blocks.3.attentions.0",
        320,
        5,
    )?;
    let up3_r1 = unet.resnet(
        &UnetFirst::concat_planar(&up3_a0, &res0),
        u2w,
        u2h,
        &temb,
        "unet.up_blocks.3.resnets.1",
        640,
    )?;
    let up3_a1 = unet.transformer2d(
        &up3_r1,
        u2w,
        u2h,
        &encoder,
        77,
        "unet.up_blocks.3.attentions.1",
        320,
        5,
    )?;
    let up3_r2 = unet.resnet(
        &UnetFirst::concat_planar(&up3_a1, &conv),
        u2w,
        u2h,
        &temb,
        "unet.up_blocks.3.resnets.2",
        640,
    )?;
    let up3_a2 = unet.transformer2d(
        &up3_r2,
        u2w,
        u2h,
        &encoder,
        77,
        "unet.up_blocks.3.attentions.2",
        320,
        5,
    )?;
    println!(
        "PBR_UNET_UP3_ATTN2 digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up3_a2),
        &up3_a2[..8]
    );
    let chained = unet.conv_head(&up3_a2, u2w, u2h)?;
    println!(
        "PBR_UNET_CHAINED_HEAD shape=4x{u2w}x{u2h} digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&chained),
        &chained[..8]
    );
    let up1_mr: Vec<f32> = up1_r0.iter().map(|v| v * 0.7 + 0.05).collect();
    let up1_mda = unet.transformer2d_extras(ExtraInputs {
        samples: &[&up1_r0, &up1_mr],
        samples_gpu: None,
        encoders: &[&encoder, &enc_mr],
        width: upw,
        height: uph,
        channels: 1280,
        heads: 20,
        prefix: "unet.up_blocks.1.attentions.0",
        flags: ExtraFlags {
            mda: true,
            ..ExtraFlags::default()
        },
        dino: None,
        ref_samples: None,
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 1,
        voxel_xyz: None,
        voxel_res: 64,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })?;
    let up1_mda_cat = UnetFirst::concat_planar(&up1_mda[0], &up1_mda[1]);
    println!(
        "PBR_UNET_UP1_MDA digest={} head={:?}",
        makepad_ai_paint::numerical_fixtures::digest_f32(&up1_mda_cat),
        &up1_mda_cat[..8]
    );
    if let Ok(path) = std::env::var("PBR_UNET_ORACLE") {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut fails = Vec::new();
        check(&mut fails, "CONV", &conv, &text, "\"head\"", 5e-3);
        check(&mut fails, "RESNET0", &res0, &text, "\"resnet0_head\"", 1e-2);
        check(&mut fails, "RESNET1", &res1, &text, "\"resnet1_head\"", 1e-3);
        check(&mut fails, "DOWN", &down, &text, "\"down_head\"", 1e-3);
        check(&mut fails, "ATTN0", &attn0, &text, "\"attn0_head\"", 1e-3);
        check(&mut fails, "ATTN1", &attn1, &text, "\"attn1_head\"", 1e-3);
        check(&mut fails, "D1_RES0", &d1_res0, &text, "\"d1_res0_head\"", 2e-3);
        check(&mut fails, "D1_RES1", &d1_res1, &text, "\"d1_res1_head\"", 2e-3);
        check(&mut fails, "D1_ATTN0", &d1_attn0, &text, "\"d1_attn0_head\"", 2e-3);
        check(&mut fails, "D1_ATTN1", &d1_attn1, &text, "\"d1_attn1_head\"", 2e-3);
        check(&mut fails, "D1_DOWN", &d1_down, &text, "\"d1_down_head\"", 5e-3);
        check(&mut fails, "D2_RES0", &d2_res0, &text, "\"d2_res0_head\"", 5e-3);
        check(&mut fails, "D2_RES1", &d2_res1, &text, "\"d2_res1_head\"", 5e-3);
        check(&mut fails, "D2_ATTN0", &d2_attn0, &text, "\"d2_attn0_head\"", 5e-3);
        check(&mut fails, "D2_DOWN", &d2_down, &text, "\"d2_down_head\"", 5e-3);
        check(&mut fails, "D3_RES0", &d3_res0, &text, "\"d3_res0_head\"", 5e-3);
        check(&mut fails, "D3_RES1", &d3_res1, &text, "\"d3_res1_head\"", 5e-3);
        check(&mut fails, "MID_RES0", &mid_res0, &text, "\"mid_res0_head\"", 5e-3);
        check(&mut fails, "MID_ATTN", &mid_attn, &text, "\"mid_attn_head\"", 5e-3);
        check(&mut fails, "MID_RES1", &mid_res1, &text, "\"mid_res1_head\"", 5e-3);
        check(&mut fails, "DINO_PROJ", &dino_tok, &text, "\"dino_proj_head\"", 1e-4);
        check(&mut fails, "MDA", &mda_cat, &text, "\"mda_head\"", 2e-3);
        check(&mut fails, "DINO", &dino_cat, &text, "\"dino_head\"", 2e-3);
        check(&mut fails, "MDA_DINO", &mda_dino_cat, &text, "\"mda_dino_head\"", 2e-3);
        check(&mut fails, "REF", &ra_cat, &text, "\"ref_head\"", 2e-3);
        if let Some(ma) = ma_out.as_ref() {
            check(&mut fails, "MA", ma, &text, "\"ma_head\"", 3e-3);
        }
        check(&mut fails, "UP0_RES0", &up0_r0, &text, "\"up0_res0_head\"", 5e-3);
        check(&mut fails, "UP0_RES1", &up0_r1, &text, "\"up0_res1_head\"", 5e-3);
        check(&mut fails, "UP0_RES2", &up0_r2, &text, "\"up0_res2_head\"", 5e-3);
        check(&mut fails, "UP0_UP", &up0_up, &text, "\"up0_up_head\"", 5e-3);
        check(&mut fails, "UP1_RES0", &up1_r0, &text, "\"up1_res0_head\"", 5e-3);
        check(&mut fails, "UP1_ATTN0", &up1_a0, &text, "\"up1_attn0_head\"", 5e-3);
        check(&mut fails, "UP1_RES1", &up1_r1, &text, "\"up1_res1_head\"", 5e-3);
        check(&mut fails, "UP1_ATTN1", &up1_a1, &text, "\"up1_attn1_head\"", 5e-3);
        check(&mut fails, "UP1_RES2", &up1_r2, &text, "\"up1_res2_head\"", 5e-3);
        check(&mut fails, "UP1_ATTN2", &up1_a2, &text, "\"up1_attn2_head\"", 5e-3);
        check(&mut fails, "UP1_UP", &up1_up, &text, "\"up1_up_head\"", 5e-3);
        check(&mut fails, "UP2_RES0", &up2_r0, &text, "\"up2_res0_head\"", 5e-3);
        check(&mut fails, "UP2_ATTN0", &up2_a0, &text, "\"up2_attn0_head\"", 5e-3);
        check(&mut fails, "UP2_RES1", &up2_r1, &text, "\"up2_res1_head\"", 5e-3);
        check(&mut fails, "UP2_ATTN1", &up2_a1, &text, "\"up2_attn1_head\"", 5e-3);
        check(&mut fails, "UP2_RES2", &up2_r2, &text, "\"up2_res2_head\"", 5e-3);
        check(&mut fails, "UP2_ATTN2", &up2_a2, &text, "\"up2_attn2_head\"", 5e-3);
        check(&mut fails, "UP2_UP", &up2_up, &text, "\"up2_up_head\"", 5e-3);
        check(&mut fails, "UP3_RES0", &up3_r0, &text, "\"up3_res0_head\"", 5e-3);
        check(&mut fails, "UP3_ATTN0", &up3_a0, &text, "\"up3_attn0_head\"", 5e-3);
        check(&mut fails, "UP3_RES1", &up3_r1, &text, "\"up3_res1_head\"", 5e-3);
        check(&mut fails, "UP3_ATTN1", &up3_a1, &text, "\"up3_attn1_head\"", 5e-3);
        check(&mut fails, "UP3_RES2", &up3_r2, &text, "\"up3_res2_head\"", 5e-3);
        check(&mut fails, "UP3_ATTN2", &up3_a2, &text, "\"up3_attn2_head\"", 5e-3);
        check(&mut fails, "UP1_MDA", &up1_mda_cat, &text, "\"up1_mda_head\"", 3e-3);
        check(&mut fails, "CHAINED_HEAD", &chained, &text, "\"conv_out_head\"", 5e-3);
        if let Ok(acts_path) = std::env::var("PBR_UNET_ACTS") {
            let acts = load_acts(&acts_path)?;
            isolated(
                &mut fails,
                "D1_RES0_ISO",
                &unet.resnet(need(&acts, "down")?, dw, dh, &temb, "unet.down_blocks.1.resnets.0", 320)?,
                need(&acts, "d1_res0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D1_RES1_ISO",
                &unet.resnet(need(&acts, "d1_res0")?, dw, dh, &temb, "unet.down_blocks.1.resnets.1", 640)?,
                need(&acts, "d1_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D1_DOWN_ISO",
                &unet.downsample(need(&acts, "d1_res1")?, dw, dh, "unet.down_blocks.1.downsamplers.0.conv", 640)?.0,
                need(&acts, "d1_down")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D2_RES0_ISO",
                &unet.resnet(need(&acts, "d1_down")?, d1w, d1h, &temb, "unet.down_blocks.2.resnets.0", 640)?,
                need(&acts, "d2_res0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D2_RES1_ISO",
                &unet.resnet(need(&acts, "d2_res0")?, d1w, d1h, &temb, "unet.down_blocks.2.resnets.1", 1280)?,
                need(&acts, "d2_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D2_DOWN_ISO",
                &unet.downsample(need(&acts, "d2_res1")?, d1w, d1h, "unet.down_blocks.2.downsamplers.0.conv", 1280)?.0,
                need(&acts, "d2_down")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "D3_RES0_ISO",
                &unet.resnet(need(&acts, "d2_down")?, d2w, d2h, &temb, "unet.down_blocks.3.resnets.0", 1280)?,
                need(&acts, "d3_res0")?,
                2e-3,
            );
            isolated(
                &mut fails,
                "D3_RES1_ISO",
                &unet.resnet(need(&acts, "d3_res0")?, d2w, d2h, &temb, "unet.down_blocks.3.resnets.1", 1280)?,
                need(&acts, "d3_res1")?,
                2e-3,
            );
            isolated(
                &mut fails,
                "MID_RES0_ISO",
                &unet.resnet(need(&acts, "d3_res1")?, d2w, d2h, &temb, "unet.mid_block.resnets.0", 1280)?,
                need(&acts, "mid_res0")?,
                2e-3,
            );
            isolated(
                &mut fails,
                "MID_ATTN_ISO",
                &unet.transformer2d(need(&acts, "mid_res0")?, d2w, d2h, &encoder, 77, "unet.mid_block.attentions.0", 1280, 20)?,
                need(&acts, "mid_attn")?,
                2e-3,
            );
            isolated(
                &mut fails,
                "MID_RES1_ISO",
                &unet.resnet(need(&acts, "mid_attn")?, d2w, d2h, &temb, "unet.mid_block.resnets.1", 1280)?,
                need(&acts, "mid_res1")?,
                2e-3,
            );
            let up0_iso = unet.resnet(
                &UnetFirst::concat_planar(need(&acts, "mid_res1")?, need(&acts, "d3_res1")?),
                d2w,
                d2h,
                &temb,
                "unet.up_blocks.0.resnets.0",
                2560,
            )?;
            isolated(&mut fails, "UP0_RES0_ISO", &up0_iso, need(&acts, "up0_res0")?, 3e-3);
            isolated(
                &mut fails,
                "UP0_RES1_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up0_res0")?, need(&acts, "d3_res0")?),
                    d2w,
                    d2h,
                    &temb,
                    "unet.up_blocks.0.resnets.1",
                    2560,
                )?,
                need(&acts, "up0_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP0_RES2_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up0_res1")?, need(&acts, "d2_down")?),
                    d2w,
                    d2h,
                    &temb,
                    "unet.up_blocks.0.resnets.2",
                    2560,
                )?,
                need(&acts, "up0_res2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP0_UP_ISO",
                &unet
                    .upsample(
                        need(&acts, "up0_res2")?,
                        d2w,
                        d2h,
                        "unet.up_blocks.0.upsamplers.0.conv",
                        1280,
                    )?
                    .0,
                need(&acts, "up0_up")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_RES0_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up0_up")?, need(&acts, "d2_res1")?),
                    upw,
                    uph,
                    &temb,
                    "unet.up_blocks.1.resnets.0",
                    2560,
                )?,
                need(&acts, "up1_res0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_ATTN0_ISO",
                &unet.transformer2d(
                    need(&acts, "up1_res0")?,
                    upw,
                    uph,
                    &encoder,
                    77,
                    "unet.up_blocks.1.attentions.0",
                    1280,
                    20,
                )?,
                need(&acts, "up1_attn0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_RES1_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up1_attn0")?, need(&acts, "d2_res0")?),
                    upw,
                    uph,
                    &temb,
                    "unet.up_blocks.1.resnets.1",
                    2560,
                )?,
                need(&acts, "up1_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_ATTN1_ISO",
                &unet.transformer2d(
                    need(&acts, "up1_res1")?,
                    upw,
                    uph,
                    &encoder,
                    77,
                    "unet.up_blocks.1.attentions.1",
                    1280,
                    20,
                )?,
                need(&acts, "up1_attn1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_RES2_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up1_attn1")?, need(&acts, "d1_down")?),
                    upw,
                    uph,
                    &temb,
                    "unet.up_blocks.1.resnets.2",
                    1920,
                )?,
                need(&acts, "up1_res2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_ATTN2_ISO",
                &unet.transformer2d(
                    need(&acts, "up1_res2")?,
                    upw,
                    uph,
                    &encoder,
                    77,
                    "unet.up_blocks.1.attentions.2",
                    1280,
                    20,
                )?,
                need(&acts, "up1_attn2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP1_UP_ISO",
                &unet
                    .upsample(
                        need(&acts, "up1_attn2")?,
                        upw,
                        uph,
                        "unet.up_blocks.1.upsamplers.0.conv",
                        1280,
                    )?
                    .0,
                need(&acts, "up1_up")?,
                5e-3,
            );
            isolated(
                &mut fails,
                "UP2_RES0_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up1_up")?, need(&acts, "d1_res1")?),
                    u1w,
                    u1h,
                    &temb,
                    "unet.up_blocks.2.resnets.0",
                    1920,
                )?,
                need(&acts, "up2_res0")?,
                5e-3,
            );
            isolated(
                &mut fails,
                "UP2_ATTN0_ISO",
                &unet.transformer2d(
                    need(&acts, "up2_res0")?,
                    u1w,
                    u1h,
                    &encoder,
                    77,
                    "unet.up_blocks.2.attentions.0",
                    640,
                    10,
                )?,
                need(&acts, "up2_attn0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP2_RES1_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up2_attn0")?, need(&acts, "d1_res0")?),
                    u1w,
                    u1h,
                    &temb,
                    "unet.up_blocks.2.resnets.1",
                    1280,
                )?,
                need(&acts, "up2_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP2_ATTN1_ISO",
                &unet.transformer2d(
                    need(&acts, "up2_res1")?,
                    u1w,
                    u1h,
                    &encoder,
                    77,
                    "unet.up_blocks.2.attentions.1",
                    640,
                    10,
                )?,
                need(&acts, "up2_attn1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP2_RES2_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up2_attn1")?, need(&acts, "down")?),
                    u1w,
                    u1h,
                    &temb,
                    "unet.up_blocks.2.resnets.2",
                    960,
                )?,
                need(&acts, "up2_res2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP2_ATTN2_ISO",
                &unet.transformer2d(
                    need(&acts, "up2_res2")?,
                    u1w,
                    u1h,
                    &encoder,
                    77,
                    "unet.up_blocks.2.attentions.2",
                    640,
                    10,
                )?,
                need(&acts, "up2_attn2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP2_UP_ISO",
                &unet
                    .upsample(
                        need(&acts, "up2_attn2")?,
                        u1w,
                        u1h,
                        "unet.up_blocks.2.upsamplers.0.conv",
                        640,
                    )?
                    .0,
                need(&acts, "up2_up")?,
                5e-3,
            );
            isolated(
                &mut fails,
                "UP3_RES0_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up2_up")?, need(&acts, "res1")?),
                    u2w,
                    u2h,
                    &temb,
                    "unet.up_blocks.3.resnets.0",
                    960,
                )?,
                need(&acts, "up3_res0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP3_ATTN0_ISO",
                &unet.transformer2d(
                    need(&acts, "up3_res0")?,
                    u2w,
                    u2h,
                    &encoder,
                    77,
                    "unet.up_blocks.3.attentions.0",
                    320,
                    5,
                )?,
                need(&acts, "up3_attn0")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP3_RES1_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up3_attn0")?, need(&acts, "res0")?),
                    u2w,
                    u2h,
                    &temb,
                    "unet.up_blocks.3.resnets.1",
                    640,
                )?,
                need(&acts, "up3_res1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP3_ATTN1_ISO",
                &unet.transformer2d(
                    need(&acts, "up3_res1")?,
                    u2w,
                    u2h,
                    &encoder,
                    77,
                    "unet.up_blocks.3.attentions.1",
                    320,
                    5,
                )?,
                need(&acts, "up3_attn1")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP3_RES2_ISO",
                &unet.resnet(
                    &UnetFirst::concat_planar(need(&acts, "up3_attn1")?, need(&acts, "conv")?),
                    u2w,
                    u2h,
                    &temb,
                    "unet.up_blocks.3.resnets.2",
                    640,
                )?,
                need(&acts, "up3_res2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "UP3_ATTN2_ISO",
                &unet.transformer2d(
                    need(&acts, "up3_res2")?,
                    u2w,
                    u2h,
                    &encoder,
                    77,
                    "unet.up_blocks.3.attentions.2",
                    320,
                    5,
                )?,
                need(&acts, "up3_attn2")?,
                3e-3,
            );
            isolated(
                &mut fails,
                "CONV_HEAD_ISO",
                &unet.conv_head(need(&acts, "up3_attn2")?, u2w, u2h)?,
                need(&acts, "conv_out")?,
                5e-3,
            );
            isolated(
                &mut fails,
                "CHAINED_HEAD_FULL",
                &chained,
                need(&acts, "conv_out")?,
                5e-3,
            );
            if let (Ok(base), Ok(expected)) = (need(&acts, "up1_res0"), need(&acts, "up1_mda")) {
                let mr: Vec<f32> = base.iter().map(|v| v * 0.7 + 0.05).collect();
                let got = unet.transformer2d_extras(ExtraInputs {
                    samples: &[base, &mr],
                    samples_gpu: None,
                    encoders: &[&encoder, &enc_mr],
                    width: upw,
                    height: uph,
                    channels: 1280,
                    heads: 20,
                    prefix: "unet.up_blocks.1.attentions.0",
                    flags: ExtraFlags {
                        mda: true,
                        ..ExtraFlags::default()
                    },
                    dino: None,
                    ref_samples: None,
        ref_tokens: None,
                    ref_scale: 1.0,
                    n_views: 1,
                    voxel_xyz: None,
                    voxel_res: 64,
                    mva_scale: 1.0,
                    dinos: None,
                    ref_scales: None,
                })?;
                isolated(
                    &mut fails,
                    "UP1_MDA_ISO",
                    &UnetFirst::concat_planar(&got[0], &got[1]),
                    expected,
                    3e-3,
                );
            }
            compare_module_chain(
                &mut fails,
                &unet,
                &acts,
                &temb,
                &encoder,
                h,
                dw,
                d1w,
                d2w,
            )?;
            compare_extras_on(
                &mut fails,
                &unet,
                &acts,
                &temb,
                &encoder,
                &enc_mr,
                &dino_tok,
                h,
                dw,
                d1w,
                d2w,
            )?;
            compare_extras_on_module(
                &mut fails,
                &unet,
                &acts,
                &temb,
                &encoder,
                &enc_mr,
                &dino_tok,
                h,
                dw,
                d1w,
                d2w,
            )?;
            compare_dual_write(&mut fails, &unet, &acts, h)?;
            compare_ddim(&mut fails, &unet, &acts, h)?;
        }
        if !fails.is_empty() {
            return Err(fails.join("; "));
        }
    }
    println!("PBR_UNET_CANARY_OK");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn affine4(x: &[f32]) -> [Vec<f32>; 4] {
    let v0 = x.to_vec();
    let v1: Vec<f32> = x.iter().map(|v| v * 0.8 + 0.02).collect();
    let mr0: Vec<f32> = x.iter().map(|v| v * 0.7 + 0.05).collect();
    let mr1: Vec<f32> = mr0.iter().map(|v| v * 0.8 + 0.02).collect();
    [v0, v1, mr0, mr1]
}

fn cat4(xs: &[Vec<f32>]) -> Vec<f32> {
    let mut out = Vec::new();
    for x in xs {
        out.extend_from_slice(x);
    }
    out
}

fn parse_voxels(flat: &[f32]) -> Vec<[u32; 3]> {
    flat.chunks_exact(3)
        .map(|c| [c[0] as u32, c[1] as u32, c[2] as u32])
        .collect()
}

fn voxel_table(
    acts: &std::collections::HashMap<String, Vec<f32>>,
    tokens: usize,
) -> Result<(Vec<[u32; 3]>, usize), String> {
    let key = format!("ma_voxel_{tokens}");
    let flat = if let Some(v) = acts.get(&key) {
        v.as_slice()
    } else if tokens == 128 {
        need(acts, "ma_voxel")?
    } else {
        return Err(format!("missing {key}"));
    };
    let xyz = parse_voxels(flat);
    if xyz.len() != tokens {
        return Err(format!("{key} n={} vs {tokens}", xyz.len()));
    }
    let res = match tokens {
        128 => 64,
        32 => 32,
        8 => 16,
        2 => 8,
        _ => return Err(format!("unknown voxel tokens {tokens}")),
    };
    Ok((xyz, res))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn extras_on_attn_refs(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    refs: &[Vec<f32>],
    width: usize,
    height: usize,
    channels: usize,
    heads: usize,
    prefix: &str,
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    voxels: &[[u32; 3]],
    voxel_res: usize,
) -> Result<Vec<Vec<f32>>, String> {
    use makepad_ai_paint::unet_extras::{ExtraFlags, ExtraInputs};
    if xs.len() != 4 || refs.len() != 2 {
        return Err(format!(
            "{prefix} extras-on module expects 4 samples + 2 refs, got {} / {}",
            xs.len(),
            refs.len()
        ));
    }
    let ref_s: [&[f32]; 2] = [&refs[0], &refs[1]];
    unet.transformer2d_extras(ExtraInputs {
        samples: &[&xs[0], &xs[1], &xs[2], &xs[3]],
        samples_gpu: None,
        encoders: &[enc_alb, enc_alb, enc_mr, enc_mr],
        width,
        height,
        channels,
        heads,
        prefix,
        flags: ExtraFlags {
            mda: true,
            dino: true,
            ra: true,
            ma: true,
        },
        dino: Some(dino),
        ref_samples: Some(&ref_s),
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 2,
        voxel_xyz: Some(voxels),
        voxel_res,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })
}

fn map_n_resnet(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    temb: &[f32],
    prefix: &str,
    cin: usize,
) -> Result<Vec<Vec<f32>>, String> {
    xs.iter()
        .map(|x| unet.resnet(x, w, h, temb, prefix, cin))
        .collect()
}

fn map_n_attn_off(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    enc: &[f32],
    prefix: &str,
    ch: usize,
    heads: usize,
) -> Result<Vec<Vec<f32>>, String> {
    xs.iter()
        .map(|x| unet.transformer2d(x, w, h, enc, 77, prefix, ch, heads))
        .collect()
}

fn map_n_down(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.downsample(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn map_n_up(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.upsample(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn write2_from(v0: &[f32], v1: &[f32]) -> Vec<Vec<f32>> {
    vec![
        v0.iter().map(|v| v * 0.5 + 0.1).collect(),
        v1.iter().map(|v| v * 0.5 + 0.1).collect(),
    ]
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn extras_on_attn(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    width: usize,
    height: usize,
    channels: usize,
    heads: usize,
    prefix: &str,
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    voxels: &[[u32; 3]],
    voxel_res: usize,
) -> Result<Vec<Vec<f32>>, String> {
    use makepad_ai_paint::unet_extras::{ExtraFlags, ExtraInputs};
    if xs.len() != 4 {
        return Err(format!("{prefix} extras-on expects 4 samples, got {}", xs.len()));
    }
    let write0: Vec<f32> = xs[0].iter().map(|v| v * 0.5 + 0.1).collect();
    let write1: Vec<f32> = xs[1].iter().map(|v| v * 0.5 + 0.1).collect();
    let refs: [&[f32]; 2] = [&write0, &write1];
    unet.transformer2d_extras(ExtraInputs {
        samples: &[&xs[0], &xs[1], &xs[2], &xs[3]],
        samples_gpu: None,
        encoders: &[enc_alb, enc_alb, enc_mr, enc_mr],
        width,
        height,
        channels,
        heads,
        prefix,
        flags: ExtraFlags {
            mda: true,
            dino: true,
            ra: true,
            ma: true,
        },
        dino: Some(dino),
        ref_samples: Some(&refs),
        ref_tokens: None,
        ref_scale: 1.0,
        n_views: 2,
        voxel_xyz: Some(voxels),
        voxel_res,
        mva_scale: 1.0,
        dinos: None,
        ref_scales: None,
    })
}

fn map4_resnet(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    temb: &[f32],
    prefix: &str,
    cin: usize,
) -> Result<Vec<Vec<f32>>, String> {
    xs.iter()
        .map(|x| unet.resnet(x, w, h, temb, prefix, cin))
        .collect()
}

fn map4_down(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<(Vec<Vec<f32>>, usize, usize), String> {
    let mut out = Vec::with_capacity(xs.len());
    let mut nw = w;
    let mut nh = h;
    for x in xs {
        let (y, ww, hh) = unet.downsample(x, w, h, prefix, ch)?;
        nw = ww;
        nh = hh;
        out.push(y);
    }
    Ok((out, nw, nh))
}

fn map4_up(
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<(Vec<Vec<f32>>, usize, usize), String> {
    let mut out = Vec::with_capacity(xs.len());
    let mut nw = w;
    let mut nh = h;
    for x in xs {
        let (y, ww, hh) = unet.upsample(x, w, h, prefix, ch)?;
        nw = ww;
        nh = hh;
        out.push(y);
    }
    Ok((out, nw, nh))
}

fn cat_skip4(hidden: &[Vec<f32>], skip: &[Vec<f32>]) -> Vec<Vec<f32>> {
    hidden
        .iter()
        .zip(skip)
        .map(|(h, s)| makepad_ai_paint::unet_first::UnetFirst::concat_planar(h, s))
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare_module_chain(
    fails: &mut Vec<String>,
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    acts: &std::collections::HashMap<String, Vec<f32>>,
    temb: &[f32],
    enc: &[f32],
    s8: usize,
    s4: usize,
    s2: usize,
    s1: usize,
) -> Result<(), String> {
    if !acts.contains_key("mod_d0out") {
        fails.push("missing module-chain acts (mod_d0out)".into());
        return Ok(());
    }
    let mut skips: Vec<Vec<f32>> = vec![need(acts, "conv")?.to_vec()];

    let mut h = need(acts, "conv")?.to_vec();
    h = unet.resnet(&h, s8, s8, temb, "unet.down_blocks.0.resnets.0", 320)?;
    isolated(fails, "MOD_D0R0", &h, need(acts, "mod_d0r0")?, 3e-3);
    h = unet.transformer2d(&h, s8, s8, enc, 77, "unet.down_blocks.0.attentions.0", 320, 5)?;
    isolated(fails, "MOD_D0A0", &h, need(acts, "mod_d0a0")?, 3e-3);
    skips.push(h.clone());
    h = unet.resnet(&h, s8, s8, temb, "unet.down_blocks.0.resnets.1", 320)?;
    isolated(fails, "MOD_D0R1", &h, need(acts, "mod_d0r1")?, 3e-3);
    h = unet.transformer2d(&h, s8, s8, enc, 77, "unet.down_blocks.0.attentions.1", 320, 5)?;
    isolated(fails, "MOD_D0A1", &h, need(acts, "mod_d0a1")?, 3e-3);
    skips.push(h.clone());
    let down0 = unet.downsample(&h, s8, s8, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    h = down0.0;
    isolated(fails, "MOD_D0OUT", &h, need(acts, "mod_d0out")?, 5e-3);
    skips.push(h.clone());

    h = unet.resnet(&h, s4, s4, temb, "unet.down_blocks.1.resnets.0", 320)?;
    isolated(fails, "MOD_D1R0", &h, need(acts, "mod_d1r0")?, 3e-3);
    h = unet.transformer2d(&h, s4, s4, enc, 77, "unet.down_blocks.1.attentions.0", 640, 10)?;
    isolated(fails, "MOD_D1A0", &h, need(acts, "mod_d1a0")?, 3e-3);
    skips.push(h.clone());
    h = unet.resnet(&h, s4, s4, temb, "unet.down_blocks.1.resnets.1", 640)?;
    isolated(fails, "MOD_D1R1", &h, need(acts, "mod_d1r1")?, 3e-3);
    h = unet.transformer2d(&h, s4, s4, enc, 77, "unet.down_blocks.1.attentions.1", 640, 10)?;
    isolated(fails, "MOD_D1A1", &h, need(acts, "mod_d1a1")?, 3e-3);
    skips.push(h.clone());
    let down1 = unet.downsample(&h, s4, s4, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    h = down1.0;
    isolated(fails, "MOD_D1OUT", &h, need(acts, "mod_d1out")?, 5e-3);
    skips.push(h.clone());

    h = unet.resnet(&h, s2, s2, temb, "unet.down_blocks.2.resnets.0", 640)?;
    isolated(fails, "MOD_D2R0", &h, need(acts, "mod_d2r0")?, 3e-3);
    h = unet.transformer2d(&h, s2, s2, enc, 77, "unet.down_blocks.2.attentions.0", 1280, 20)?;
    isolated(fails, "MOD_D2A0", &h, need(acts, "mod_d2a0")?, 3e-3);
    skips.push(h.clone());
    h = unet.resnet(&h, s2, s2, temb, "unet.down_blocks.2.resnets.1", 1280)?;
    isolated(fails, "MOD_D2R1", &h, need(acts, "mod_d2r1")?, 3e-3);
    h = unet.transformer2d(&h, s2, s2, enc, 77, "unet.down_blocks.2.attentions.1", 1280, 20)?;
    isolated(fails, "MOD_D2A1", &h, need(acts, "mod_d2a1")?, 3e-3);
    skips.push(h.clone());
    let down2 = unet.downsample(&h, s2, s2, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    h = down2.0;
    isolated(fails, "MOD_D2OUT", &h, need(acts, "mod_d2out")?, 5e-3);
    skips.push(h.clone());

    h = unet.resnet(&h, s1, s1, temb, "unet.down_blocks.3.resnets.0", 1280)?;
    isolated(fails, "MOD_D3R0", &h, need(acts, "mod_d3r0")?, 3e-3);
    skips.push(h.clone());
    h = unet.resnet(&h, s1, s1, temb, "unet.down_blocks.3.resnets.1", 1280)?;
    isolated(fails, "MOD_D3R1", &h, need(acts, "mod_d3r1")?, 3e-3);
    skips.push(h.clone());

    h = unet.resnet(&h, s1, s1, temb, "unet.mid_block.resnets.0", 1280)?;
    isolated(fails, "MOD_MIDR0", &h, need(acts, "mod_midr0")?, 3e-3);
    h = unet.transformer2d(&h, s1, s1, enc, 77, "unet.mid_block.attentions.0", 1280, 20)?;
    isolated(fails, "MOD_MIDA", &h, need(acts, "mod_mida")?, 5e-3);
    h = unet.resnet(&h, s1, s1, temb, "unet.mid_block.resnets.1", 1280)?;
    isolated(fails, "MOD_MIDR1", &h, need(acts, "mod_midr1")?, 5e-3);

    // up0: UpBlock2D, 3 resnets, skip order LIFO = d3r1, d3r0, d2out
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s1, s1, temb, "unet.up_blocks.0.resnets.0", 2560,
    )?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s1, s1, temb, "unet.up_blocks.0.resnets.1", 2560,
    )?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s1, s1, temb, "unet.up_blocks.0.resnets.2", 2560,
    )?;
    let up0 = unet.upsample(&h, s1, s1, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;
    h = up0.0;
    isolated(fails, "MOD_UP0", &h, need(acts, "mod_up0")?, 5e-3);

    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s2, s2, temb, "unet.up_blocks.1.resnets.0", 2560,
    )?;
    h = unet.transformer2d(&h, s2, s2, enc, 77, "unet.up_blocks.1.attentions.0", 1280, 20)?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s2, s2, temb, "unet.up_blocks.1.resnets.1", 2560,
    )?;
    h = unet.transformer2d(&h, s2, s2, enc, 77, "unet.up_blocks.1.attentions.1", 1280, 20)?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s2, s2, temb, "unet.up_blocks.1.resnets.2", 1920,
    )?;
    h = unet.transformer2d(&h, s2, s2, enc, 77, "unet.up_blocks.1.attentions.2", 1280, 20)?;
    let up1 = unet.upsample(&h, s2, s2, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;
    h = up1.0;
    isolated(fails, "MOD_UP1", &h, need(acts, "mod_up1")?, 2e-2);

    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s4, s4, temb, "unet.up_blocks.2.resnets.0", 1920,
    )?;
    h = unet.transformer2d(&h, s4, s4, enc, 77, "unet.up_blocks.2.attentions.0", 640, 10)?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s4, s4, temb, "unet.up_blocks.2.resnets.1", 1280,
    )?;
    h = unet.transformer2d(&h, s4, s4, enc, 77, "unet.up_blocks.2.attentions.1", 640, 10)?;
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s4, s4, temb, "unet.up_blocks.2.resnets.2", 960,
    )?;
    h = unet.transformer2d(&h, s4, s4, enc, 77, "unet.up_blocks.2.attentions.2", 640, 10)?;
    let up2 = unet.upsample(&h, s4, s4, "unet.up_blocks.2.upsamplers.0.conv", 640)?;
    h = up2.0;
    isolated(fails, "MOD_UP2", &h, need(acts, "mod_up2")?, 2e-2);

    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s8, s8, temb, "unet.up_blocks.3.resnets.0", 960,
    )?;
    h = unet.transformer2d(&h, s8, s8, enc, 77, "unet.up_blocks.3.attentions.0", 320, 5)?;
    isolated(fails, "MOD_U3A0", &h, need(acts, "mod_u3a0")?, 5e-3);
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s8, s8, temb, "unet.up_blocks.3.resnets.1", 640,
    )?;
    h = unet.transformer2d(&h, s8, s8, enc, 77, "unet.up_blocks.3.attentions.1", 320, 5)?;
    isolated(fails, "MOD_U3A1", &h, need(acts, "mod_u3a1")?, 5e-3);
    h = unet.resnet(
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&h, &skips.pop().unwrap()),
        s8, s8, temb, "unet.up_blocks.3.resnets.2", 640,
    )?;
    h = unet.transformer2d(&h, s8, s8, enc, 77, "unet.up_blocks.3.attentions.2", 320, 5)?;
    isolated(fails, "MOD_U3A2", &h, need(acts, "mod_u3a2")?, 5e-3);
    isolated(fails, "MOD_U3OUT", &h, need(acts, "mod_u3out")?, 5e-3);

    let head = unet.conv_head(&h, s8, s8)?;
    isolated(fails, "MOD_HEAD", &head, need(acts, "mod_head")?, 5e-3);
    if !skips.is_empty() {
        fails.push(format!("module-chain skip leftover {}", skips.len()));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare_extras_on_module(
    fails: &mut Vec<String>,
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    acts: &std::collections::HashMap<String, Vec<f32>>,
    temb: &[f32],
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    s8: usize,
    s4: usize,
    s2: usize,
    s1: usize,
) -> Result<(), String> {
    if !acts.contains_key("xom_d0out") {
        fails.push("missing extras-on module-chain acts (xom_d0out)".into());
        return Ok(());
    }
    let (vox128, res128) = voxel_table(acts, 2 * s8 * s8)?;
    let (vox32, res32) = voxel_table(acts, 2 * s4 * s4)?;
    let (vox8, res8) = voxel_table(acts, 2 * s2 * s2)?;
    let (vox2, res2) = voxel_table(acts, 2 * s1 * s1)?;

    let pack = affine4(need(acts, "conv")?);
    let mut w = write2_from(&pack[0], &pack[1]);
    let mut r = pack.to_vec();
    let mut r_skips: Vec<Vec<Vec<f32>>> = vec![r.clone()];
    let mut w_skips: Vec<Vec<Vec<f32>>> = vec![w.clone()];

    // down0 write then read
    w = map_n_resnet(unet, &w, s8, s8, temb, "unet.down_blocks.0.resnets.0", 320)?;
    let d0a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s8, s8, enc_alb, "unet.down_blocks.0.attentions.0", 320, 5)?;
    w_skips.push(w.clone());
    w = map_n_resnet(unet, &w, s8, s8, temb, "unet.down_blocks.0.resnets.1", 320)?;
    let d0a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s8, s8, enc_alb, "unet.down_blocks.0.attentions.1", 320, 5)?;
    w_skips.push(w.clone());
    w = map_n_down(unet, &w, s8, s8, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    w_skips.push(w.clone());

    r = map_n_resnet(unet, &r, s8, s8, temb, "unet.down_blocks.0.resnets.0", 320)?;
    r = extras_on_attn_refs(
        unet, &r, &d0a0_refs, s8, s8, 320, 5, "unet.down_blocks.0.attentions.0",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XOM_D0A0", &cat4(&r), need(acts, "xom_d0a0")?, 5e-3);
    r_skips.push(r.clone());
    r = map_n_resnet(unet, &r, s8, s8, temb, "unet.down_blocks.0.resnets.1", 320)?;
    r = extras_on_attn_refs(
        unet, &r, &d0a1_refs, s8, s8, 320, 5, "unet.down_blocks.0.attentions.1",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XOM_D0A1", &cat4(&r), need(acts, "xom_d0a1")?, 5e-3);
    r_skips.push(r.clone());
    r = map_n_down(unet, &r, s8, s8, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    isolated(fails, "XOM_D0OUT", &cat4(&r), need(acts, "xom_d0out")?, 5e-3);
    r_skips.push(r.clone());

    // down1
    w = map_n_resnet(unet, &w, s4, s4, temb, "unet.down_blocks.1.resnets.0", 320)?;
    let d1a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s4, s4, enc_alb, "unet.down_blocks.1.attentions.0", 640, 10)?;
    w_skips.push(w.clone());
    w = map_n_resnet(unet, &w, s4, s4, temb, "unet.down_blocks.1.resnets.1", 640)?;
    let d1a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s4, s4, enc_alb, "unet.down_blocks.1.attentions.1", 640, 10)?;
    w_skips.push(w.clone());
    w = map_n_down(unet, &w, s4, s4, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    w_skips.push(w.clone());

    r = map_n_resnet(unet, &r, s4, s4, temb, "unet.down_blocks.1.resnets.0", 320)?;
    r = extras_on_attn_refs(
        unet, &r, &d1a0_refs, s4, s4, 640, 10, "unet.down_blocks.1.attentions.0",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    isolated(fails, "XOM_D1A0", &cat4(&r), need(acts, "xom_d1a0")?, 5e-3);
    r_skips.push(r.clone());
    r = map_n_resnet(unet, &r, s4, s4, temb, "unet.down_blocks.1.resnets.1", 640)?;
    r = extras_on_attn_refs(
        unet, &r, &d1a1_refs, s4, s4, 640, 10, "unet.down_blocks.1.attentions.1",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    isolated(fails, "XOM_D1A1", &cat4(&r), need(acts, "xom_d1a1")?, 5e-3);
    r_skips.push(r.clone());
    r = map_n_down(unet, &r, s4, s4, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    isolated(fails, "XOM_D1OUT", &cat4(&r), need(acts, "xom_d1out")?, 5e-3);
    r_skips.push(r.clone());

    // down2
    w = map_n_resnet(unet, &w, s2, s2, temb, "unet.down_blocks.2.resnets.0", 640)?;
    let d2a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s2, s2, enc_alb, "unet.down_blocks.2.attentions.0", 1280, 20)?;
    w_skips.push(w.clone());
    w = map_n_resnet(unet, &w, s2, s2, temb, "unet.down_blocks.2.resnets.1", 1280)?;
    let d2a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s2, s2, enc_alb, "unet.down_blocks.2.attentions.1", 1280, 20)?;
    w_skips.push(w.clone());
    w = map_n_down(unet, &w, s2, s2, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    w_skips.push(w.clone());

    r = map_n_resnet(unet, &r, s2, s2, temb, "unet.down_blocks.2.resnets.0", 640)?;
    r = extras_on_attn_refs(
        unet, &r, &d2a0_refs, s2, s2, 1280, 20, "unet.down_blocks.2.attentions.0",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    isolated(fails, "XOM_D2A0", &cat4(&r), need(acts, "xom_d2a0")?, 5e-3);
    r_skips.push(r.clone());
    r = map_n_resnet(unet, &r, s2, s2, temb, "unet.down_blocks.2.resnets.1", 1280)?;
    r = extras_on_attn_refs(
        unet, &r, &d2a1_refs, s2, s2, 1280, 20, "unet.down_blocks.2.attentions.1",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    r_skips.push(r.clone());
    r = map_n_down(unet, &r, s2, s2, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    isolated(fails, "XOM_D2OUT", &cat4(&r), need(acts, "xom_d2out")?, 5e-3);
    r_skips.push(r.clone());

    // down3 (no attn)
    w = map_n_resnet(unet, &w, s1, s1, temb, "unet.down_blocks.3.resnets.0", 1280)?;
    w_skips.push(w.clone());
    w = map_n_resnet(unet, &w, s1, s1, temb, "unet.down_blocks.3.resnets.1", 1280)?;
    w_skips.push(w.clone());
    r = map_n_resnet(unet, &r, s1, s1, temb, "unet.down_blocks.3.resnets.0", 1280)?;
    r_skips.push(r.clone());
    r = map_n_resnet(unet, &r, s1, s1, temb, "unet.down_blocks.3.resnets.1", 1280)?;
    isolated(fails, "XOM_D3R1", &cat4(&r), need(acts, "xom_d3r1")?, 5e-3);
    r_skips.push(r.clone());

    // mid
    w = map_n_resnet(unet, &w, s1, s1, temb, "unet.mid_block.resnets.0", 1280)?;
    let mid_refs = w.clone();
    w = map_n_attn_off(unet, &w, s1, s1, enc_alb, "unet.mid_block.attentions.0", 1280, 20)?;
    w = map_n_resnet(unet, &w, s1, s1, temb, "unet.mid_block.resnets.1", 1280)?;
    r = map_n_resnet(unet, &r, s1, s1, temb, "unet.mid_block.resnets.0", 1280)?;
    r = extras_on_attn_refs(
        unet, &r, &mid_refs, s1, s1, 1280, 20, "unet.mid_block.attentions.0",
        enc_alb, enc_mr, dino, &vox2, res2,
    )?;
    isolated(fails, "XOM_MIDA", &cat4(&r), need(acts, "xom_mida")?, 1e-2);
    r = map_n_resnet(unet, &r, s1, s1, temb, "unet.mid_block.resnets.1", 1280)?;
    isolated(fails, "XOM_MIDR1", &cat4(&r), need(acts, "xom_midr1")?, 5e-3);

    if !acts.contains_key("xom_head") {
        return Ok(());
    }

    // up0 write/read — write path still needed so refs for later up attns stay aligned
    let pop = |skips: &mut Vec<Vec<Vec<f32>>>| skips.pop().unwrap();
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.0", 2560)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.1", 2560)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.2", 2560)?;
    w = map_n_up(unet, &w, s1, s1, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.0", 2560)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.1", 2560)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s1, s1, temb, "unet.up_blocks.0.resnets.2", 2560)?;
    r = map_n_up(unet, &r, s1, s1, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;
    isolated(fails, "XOM_UP0", &cat4(&r), need(acts, "xom_up0")?, 3e-2);

    // Remaining up write/read is optional for the down-block pass bar; still compare head if present.
    // Continue write+read so RA caches stay aligned with official full-module write.
    // up1
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.0", 2560)?;
    let u1a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s2, s2, enc_alb, "unet.up_blocks.1.attentions.0", 1280, 20)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.1", 2560)?;
    let u1a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s2, s2, enc_alb, "unet.up_blocks.1.attentions.1", 1280, 20)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.2", 1920)?;
    let u1a2_refs = w.clone();
    w = map_n_attn_off(unet, &w, s2, s2, enc_alb, "unet.up_blocks.1.attentions.2", 1280, 20)?;
    w = map_n_up(unet, &w, s2, s2, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.0", 2560)?;
    r = extras_on_attn_refs(
        unet, &r, &u1a0_refs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.0",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.1", 2560)?;
    r = extras_on_attn_refs(
        unet, &r, &u1a1_refs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.1",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s2, s2, temb, "unet.up_blocks.1.resnets.2", 1920)?;
    r = extras_on_attn_refs(
        unet, &r, &u1a2_refs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.2",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    r = map_n_up(unet, &r, s2, s2, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;
    isolated(fails, "XOM_UP1", &cat4(&r), need(acts, "xom_up1")?, 4e-2);

    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.0", 1920)?;
    let u2a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s4, s4, enc_alb, "unet.up_blocks.2.attentions.0", 640, 10)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.1", 1280)?;
    let u2a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s4, s4, enc_alb, "unet.up_blocks.2.attentions.1", 640, 10)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.2", 960)?;
    let u2a2_refs = w.clone();
    w = map_n_attn_off(unet, &w, s4, s4, enc_alb, "unet.up_blocks.2.attentions.2", 640, 10)?;
    w = map_n_up(unet, &w, s4, s4, "unet.up_blocks.2.upsamplers.0.conv", 640)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.0", 1920)?;
    r = extras_on_attn_refs(
        unet, &r, &u2a0_refs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.0",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.1", 1280)?;
    r = extras_on_attn_refs(
        unet, &r, &u2a1_refs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.1",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s4, s4, temb, "unet.up_blocks.2.resnets.2", 960)?;
    r = extras_on_attn_refs(
        unet, &r, &u2a2_refs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.2",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    r = map_n_up(unet, &r, s4, s4, "unet.up_blocks.2.upsamplers.0.conv", 640)?;
    isolated(fails, "XOM_UP2", &cat4(&r), need(acts, "xom_up2")?, 3e-2);

    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.0", 960)?;
    let u3a0_refs = w.clone();
    w = map_n_attn_off(unet, &w, s8, s8, enc_alb, "unet.up_blocks.3.attentions.0", 320, 5)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.1", 640)?;
    let u3a1_refs = w.clone();
    w = map_n_attn_off(unet, &w, s8, s8, enc_alb, "unet.up_blocks.3.attentions.1", 320, 5)?;
    w = map_n_resnet(unet, &cat_skip4(&w, &pop(&mut w_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.2", 640)?;
    let u3a2_refs = w.clone();
    let _ = map_n_attn_off(unet, &w, s8, s8, enc_alb, "unet.up_blocks.3.attentions.2", 320, 5)?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.0", 960)?;
    r = extras_on_attn_refs(
        unet, &r, &u3a0_refs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.0",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.1", 640)?;
    r = extras_on_attn_refs(
        unet, &r, &u3a1_refs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.1",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    r = map_n_resnet(unet, &cat_skip4(&r, &pop(&mut r_skips)), s8, s8, temb, "unet.up_blocks.3.resnets.2", 640)?;
    r = extras_on_attn_refs(
        unet, &r, &u3a2_refs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.2",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XOM_U3OUT", &cat4(&r), need(acts, "xom_u3out")?, 3e-2);

    let heads: Result<Vec<Vec<f32>>, String> = r.iter().map(|x| unet.conv_head(x, s8, s8)).collect();
    isolated(fails, "XOM_HEAD", &cat4(&heads?), need(acts, "xom_head")?, 5e-3);
    let _ = w_skips;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare_extras_on(
    fails: &mut Vec<String>,
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    acts: &std::collections::HashMap<String, Vec<f32>>,
    temb: &[f32],
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    s8: usize,
    s4: usize,
    s2: usize,
    s1: usize,
) -> Result<(), String> {
    if !acts.contains_key("xod0a0") {
        fails.push("missing extras-on acts (xod0a0)".into());
        return Ok(());
    }
    let (vox128, res128) = voxel_table(acts, 2 * s8 * s8)?;
    let (vox32, res32) = voxel_table(acts, 2 * s4 * s4)?;
    let (vox8, res8) = voxel_table(acts, 2 * s2 * s2)?;
    let (vox2, res2) = voxel_table(acts, 2 * s1 * s1)?;

    let pack = affine4(need(acts, "res0")?);
    let got = extras_on_attn(
        unet,
        &pack,
        s8,
        s8,
        320,
        5,
        "unet.down_blocks.0.attentions.0",
        enc_alb,
        enc_mr,
        dino,
        &vox128,
        res128,
    )?;
    isolated(fails, "XOD0A0_ISO", &cat4(&got), need(acts, "xod0a0")?, 3e-3);

    let pack = affine4(need(acts, "d1_res0")?);
    let got = extras_on_attn(
        unet,
        &pack,
        s4,
        s4,
        640,
        10,
        "unet.down_blocks.1.attentions.0",
        enc_alb,
        enc_mr,
        dino,
        &vox32,
        res32,
    )?;
    isolated(fails, "XOD1A0_ISO", &cat4(&got), need(acts, "xod1a0")?, 3e-3);

    let pack = affine4(need(acts, "mid_res0")?);
    let got = extras_on_attn(
        unet,
        &pack,
        s1,
        s1,
        1280,
        20,
        "unet.mid_block.attentions.0",
        enc_alb,
        enc_mr,
        dino,
        &vox2,
        res2,
    )?;
    isolated(fails, "XOMID_ISO", &cat4(&got), need(acts, "xomid")?, 3e-3);

    if acts.contains_key("xou1a0") {
        let pack = affine4(need(acts, "up1_res0")?);
        let got = extras_on_attn(
            unet,
            &pack,
            s2,
            s2,
            1280,
            20,
            "unet.up_blocks.1.attentions.0",
            enc_alb,
            enc_mr,
            dino,
            &vox8,
            res8,
        )?;
        isolated(fails, "XOU1A0_ISO", &cat4(&got), need(acts, "xou1a0")?, 3e-3);
    }

    if !acts.contains_key("xon_head") {
        fails.push("missing extras-on chain act xon_head".into());
        return Ok(());
    }

    let mut xs = affine4(need(acts, "conv")?).to_vec();
    isolated(fails, "XON_CONV_ISO", &cat4(&xs), need(acts, "xon_conv")?, 3e-3);
    let mut skips: Vec<Vec<Vec<f32>>> = vec![xs.clone()];

    xs = map4_resnet(unet, &xs, s8, s8, temb, "unet.down_blocks.0.resnets.0", 320)?;
    xs = extras_on_attn(
        unet, &xs, s8, s8, 320, 5, "unet.down_blocks.0.attentions.0",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XON_D0A0", &cat4(&xs), need(acts, "xon_d0a0")?, 5e-3);
    skips.push(xs.clone());
    xs = map4_resnet(unet, &xs, s8, s8, temb, "unet.down_blocks.0.resnets.1", 320)?;
    xs = extras_on_attn(
        unet, &xs, s8, s8, 320, 5, "unet.down_blocks.0.attentions.1",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XON_D0A1", &cat4(&xs), need(acts, "xon_d0a1")?, 5e-3);
    skips.push(xs.clone());
    let down0 = map4_down(unet, &xs, s8, s8, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    xs = down0.0;
    isolated(fails, "XON_D0DOWN", &cat4(&xs), need(acts, "xon_d0down")?, 5e-3);
    skips.push(xs.clone());

    xs = map4_resnet(unet, &xs, s4, s4, temb, "unet.down_blocks.1.resnets.0", 320)?;
    xs = extras_on_attn(
        unet, &xs, s4, s4, 640, 10, "unet.down_blocks.1.attentions.0",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    isolated(fails, "XON_D1A0", &cat4(&xs), need(acts, "xon_d1a0")?, 5e-3);
    skips.push(xs.clone());
    xs = map4_resnet(unet, &xs, s4, s4, temb, "unet.down_blocks.1.resnets.1", 640)?;
    xs = extras_on_attn(
        unet, &xs, s4, s4, 640, 10, "unet.down_blocks.1.attentions.1",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    skips.push(xs.clone());
    let down1 = map4_down(unet, &xs, s4, s4, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    xs = down1.0;
    isolated(fails, "XON_D1DOWN", &cat4(&xs), need(acts, "xon_d1down")?, 5e-3);
    skips.push(xs.clone());

    xs = map4_resnet(unet, &xs, s2, s2, temb, "unet.down_blocks.2.resnets.0", 640)?;
    xs = extras_on_attn(
        unet, &xs, s2, s2, 1280, 20, "unet.down_blocks.2.attentions.0",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    isolated(fails, "XON_D2A0", &cat4(&xs), need(acts, "xon_d2a0")?, 5e-3);
    skips.push(xs.clone());
    xs = map4_resnet(unet, &xs, s2, s2, temb, "unet.down_blocks.2.resnets.1", 1280)?;
    xs = extras_on_attn(
        unet, &xs, s2, s2, 1280, 20, "unet.down_blocks.2.attentions.1",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    skips.push(xs.clone());
    let down2 = map4_down(unet, &xs, s2, s2, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    xs = down2.0;
    isolated(fails, "XON_D2DOWN", &cat4(&xs), need(acts, "xon_d2down")?, 5e-3);
    skips.push(xs.clone());

    xs = map4_resnet(unet, &xs, s1, s1, temb, "unet.down_blocks.3.resnets.0", 1280)?;
    skips.push(xs.clone());
    xs = map4_resnet(unet, &xs, s1, s1, temb, "unet.down_blocks.3.resnets.1", 1280)?;
    isolated(fails, "XON_D3R1", &cat4(&xs), need(acts, "xon_d3r1")?, 5e-3);
    skips.push(xs.clone());

    xs = map4_resnet(unet, &xs, s1, s1, temb, "unet.mid_block.resnets.0", 1280)?;
    xs = extras_on_attn(
        unet, &xs, s1, s1, 1280, 20, "unet.mid_block.attentions.0",
        enc_alb, enc_mr, dino, &vox2, res2,
    )?;
    isolated(fails, "XON_MID", &cat4(&xs), need(acts, "xon_mid")?, 5e-3);
    xs = map4_resnet(unet, &xs, s1, s1, temb, "unet.mid_block.resnets.1", 1280)?;
    isolated(fails, "XON_MIDR1", &cat4(&xs), need(acts, "xon_midr1")?, 5e-3);

    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s1, s1, temb, "unet.up_blocks.0.resnets.0", 2560)?;
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s1, s1, temb, "unet.up_blocks.0.resnets.1", 2560)?;
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s1, s1, temb, "unet.up_blocks.0.resnets.2", 2560)?;
    let up0 = map4_up(unet, &xs, s1, s1, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;
    xs = up0.0;
    isolated(fails, "XON_UP0", &cat4(&xs), need(acts, "xon_up0")?, 3e-2);

    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s2, s2, temb, "unet.up_blocks.1.resnets.0", 2560)?;
    xs = extras_on_attn(
        unet, &xs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.0",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    isolated(fails, "XON_U1A0", &cat4(&xs), need(acts, "xon_u1a0")?, 5e-3);
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s2, s2, temb, "unet.up_blocks.1.resnets.1", 2560)?;
    xs = extras_on_attn(
        unet, &xs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.1",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s2, s2, temb, "unet.up_blocks.1.resnets.2", 1920)?;
    xs = extras_on_attn(
        unet, &xs, s2, s2, 1280, 20, "unet.up_blocks.1.attentions.2",
        enc_alb, enc_mr, dino, &vox8, res8,
    )?;
    let up1 = map4_up(unet, &xs, s2, s2, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;
    xs = up1.0;
    isolated(fails, "XON_UP1", &cat4(&xs), need(acts, "xon_up1")?, 3e-2);

    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s4, s4, temb, "unet.up_blocks.2.resnets.0", 1920)?;
    xs = extras_on_attn(
        unet, &xs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.0",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    isolated(fails, "XON_U2A0", &cat4(&xs), need(acts, "xon_u2a0")?, 3e-2);
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s4, s4, temb, "unet.up_blocks.2.resnets.1", 1280)?;
    xs = extras_on_attn(
        unet, &xs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.1",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s4, s4, temb, "unet.up_blocks.2.resnets.2", 960)?;
    xs = extras_on_attn(
        unet, &xs, s4, s4, 640, 10, "unet.up_blocks.2.attentions.2",
        enc_alb, enc_mr, dino, &vox32, res32,
    )?;
    let up2 = map4_up(unet, &xs, s4, s4, "unet.up_blocks.2.upsamplers.0.conv", 640)?;
    xs = up2.0;
    isolated(fails, "XON_UP2", &cat4(&xs), need(acts, "xon_up2")?, 3e-2);

    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s8, s8, temb, "unet.up_blocks.3.resnets.0", 960)?;
    xs = extras_on_attn(
        unet, &xs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.0",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XON_U3A0", &cat4(&xs), need(acts, "xon_u3a0")?, 3e-2);
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s8, s8, temb, "unet.up_blocks.3.resnets.1", 640)?;
    xs = extras_on_attn(
        unet, &xs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.1",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    xs = map4_resnet(unet, &cat_skip4(&xs, &skips.pop().unwrap()), s8, s8, temb, "unet.up_blocks.3.resnets.2", 640)?;
    xs = extras_on_attn(
        unet, &xs, s8, s8, 320, 5, "unet.up_blocks.3.attentions.2",
        enc_alb, enc_mr, dino, &vox128, res128,
    )?;
    isolated(fails, "XON_U3A2", &cat4(&xs), need(acts, "xon_u3a2")?, 5e-3);

    let heads: Result<Vec<Vec<f32>>, String> = xs
        .iter()
        .map(|x| unet.conv_head(x, s8, s8))
        .collect();
    isolated(fails, "XON_HEAD", &cat4(&heads?), need(acts, "xon_head")?, 5e-3);
    if !skips.is_empty() {
        fails.push(format!("extras-on skip leftover {}", skips.len()));
    }
    Ok(())
}

fn dual_ref_latents(h: usize) -> (Vec<f32>, Vec<f32>) {
    let n4 = 4 * h * h;
    let mut v0 = vec![0.0f32; n4];
    for c in 0..4 {
        for y in 0..h {
            for x in 0..h {
                let nchw = (c * h + y) * h + x;
                v0[c * h * h + y * h + x] = nchw as f32 / n4 as f32;
            }
        }
    }
    let v1: Vec<f32> = v0.iter().map(|v| v * 0.8 + 0.02).collect();
    (v0, v1)
}

fn remap_dump_dual_cache(
    acts: &std::collections::HashMap<String, Vec<f32>>,
) -> std::collections::HashMap<String, Vec<f32>> {
    let mut out = std::collections::HashMap::new();
    for name in makepad_ai_paint::dual_stream::write_layer_names() {
        if let Some(v) = acts.get(&format!("dual_{name}")).or_else(|| acts.get(name)) {
            out.insert(name.to_string(), v.clone());
        }
    }
    out
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare_dual_write(
    fails: &mut Vec<String>,
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    acts: &std::collections::HashMap<String, Vec<f32>>,
    s8: usize,
) -> Result<(), String> {
    if !acts.contains_key("dual_down_0_0_0") {
        fails.push("missing dual write-cache acts (dual_down_0_0_0)".into());
        return Ok(());
    }
    let (v0, v1) = dual_ref_latents(s8);
    let conv0 = unet.conv_in_dual(&v0, s8, s8)?;
    let conv1 = unet.conv_in_dual(&v1, s8, s8)?;
    isolated(
        fails,
        "DUAL_CONV",
        &makepad_ai_paint::unet_first::UnetFirst::concat_planar(&conv0, &conv1),
        need(acts, "dual_conv")?,
        5e-3,
    );
    let temb = unet.timestep_embedding_named(0.0, "unet_dual.time_embedding")?;
    isolated(fails, "DUAL_TEMB", &temb, need(acts, "dual_temb")?, 5e-3);
    let cache = makepad_ai_paint::unet_forward::write_dual_cache(unet, &[&v0, &v1], s8, s8)?;
    for name in makepad_ai_paint::dual_stream::write_layer_names() {
        let tag = format!("DUAL_{}", name.to_ascii_uppercase());
        let got = cache
            .get(name)
            .ok_or_else(|| format!("write_dual_cache missing {name}"))?;
        isolated(fails, &tag, got, need(acts, &format!("dual_{name}"))?, 5e-3);
    }
    Ok(())
}

fn ddim_noise_rows(h: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let n4 = 4 * h * h;
    let mut base = vec![0.0f32; n4];
    for c in 0..4 {
        for y in 0..h {
            for x in 0..h {
                let nchw = (c * h + y) * h + x;
                base[c * h * h + y * h + x] = nchw as f32 / n4 as f32;
            }
        }
    }
    let alb1: Vec<f32> = base.iter().map(|v| v * 0.8 + 0.02).collect();
    let mr0: Vec<f32> = base.iter().map(|v| v * 0.7 + 0.05).collect();
    let mr1: Vec<f32> = mr0.iter().map(|v| v * 0.8 + 0.02).collect();
    let n0: Vec<f32> = base.iter().map(|v| v * 0.25 + 0.1).collect();
    let n1: Vec<f32> = n0.iter().map(|v| v * 0.8 + 0.02).collect();
    let p0: Vec<f32> = base.iter().map(|v| v * 0.5 - 0.2).collect();
    let p1: Vec<f32> = p0.iter().map(|v| v * 0.8 + 0.02).collect();
    (base, alb1, mr0, mr1, n0, n1, p0, p1)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare_ddim(
    fails: &mut Vec<String>,
    unet: &makepad_ai_paint::unet_first::UnetFirst,
    acts: &std::collections::HashMap<String, Vec<f32>>,
    s8: usize,
) -> Result<(), String> {
    use makepad_ai_paint::cond_assembly::pack_view_latent;
    use makepad_ai_paint::denoise::DenoiseBatch;
    use makepad_ai_paint::schedule::DdimVpredZsnr;
    use makepad_ai_paint::unet_forward::{
        forward_extras_on, predict_v_cfg_three_branch, VoxelLevel, VoxelPyramid,
    };

    if !acts.contains_key("ddim_v0") {
        fails.push("missing ddim acts (ddim_v0)".into());
        return Ok(());
    }
    let (vox128, res128) = voxel_table(acts, 2 * s8 * s8)?;
    let (vox32, res32) = voxel_table(acts, 2 * (s8 / 2) * (s8 / 2))?;
    let (vox8, res8) = voxel_table(acts, 2 * (s8 / 4) * (s8 / 4))?;
    let (vox2, res2) = voxel_table(acts, 2 * (s8 / 8) * (s8 / 8))?;
    let voxels = VoxelPyramid {
        full: VoxelLevel {
            xyz: &vox128,
            res: res128,
        },
        half: VoxelLevel {
            xyz: &vox32,
            res: res32,
        },
        quarter: VoxelLevel {
            xyz: &vox8,
            res: res8,
        },
        eighth: VoxelLevel {
            xyz: &vox2,
            res: res2,
        },
    };
    let cache = remap_dump_dual_cache(acts);
    let (alb0, alb1, mr0, mr1, n0, n1, p0, p1) = ddim_noise_rows(s8);
    let hw = s8 * s8;
    let x12s = [
        pack_view_latent(&alb0, &n0, &p0, hw),
        pack_view_latent(&alb1, &n1, &p1, hw),
        pack_view_latent(&mr0, &n0, &p0, hw),
        pack_view_latent(&mr1, &n1, &p1, hw),
    ];
    let packs: [&[f32]; 4] = [&x12s[0], &x12s[1], &x12s[2], &x12s[3]];
    let enc_alb = unet.learned_text_clip_albedo()?;
    let enc_mr = unet.learned_text_clip_mr()?;
    let dino = need(acts, "ddim_dino")
        .or_else(|_| need(acts, "dino_tok"))
        .map(|s| s.to_vec())
        .unwrap_or_else(|_| vec![0.0; 4 * 1024]);
    let temb999 = unet.timestep_embedding(999.0)?;
    let v0 = forward_extras_on(
        unet, &packs, &temb999, &enc_alb, &enc_mr, &dino, &cache, &voxels, s8, s8, 1.0,
    )?;
    isolated(fails, "DDIM_V0", &cat4(&v0), need(acts, "ddim_v0")?, 5e-3);

    let sched = DdimVpredZsnr::hunyuan_paint();
    let mut sample = alb0.clone();
    sample.extend_from_slice(&alb1);
    sample.extend_from_slice(&mr0);
    sample.extend_from_slice(&mr1);
    let mut batch = DenoiseBatch {
        sample,
        n_views: 2,
        lat_w: s8,
        lat_h: s8,
        steps: 15,
        guidance: 3.0,
        timesteps: sched.timesteps_trailing(15),
        view_scales: vec![1.0, 2.0, 1.0, 2.0],
    };
    isolated(fails, "DDIM_X0", &batch.sample, need(acts, "ddim_x0")?, 1e-6);

    let ts = batch.timesteps.clone();
    for (i, &t) in ts.iter().enumerate() {
        let row = 4 * hw;
        let packs = [
            pack_view_latent(&batch.sample[0..row], &n0, &p0, hw),
            pack_view_latent(&batch.sample[row..2 * row], &n1, &p1, hw),
            pack_view_latent(&batch.sample[2 * row..3 * row], &n0, &p0, hw),
            pack_view_latent(&batch.sample[3 * row..4 * row], &n1, &p1, hw),
        ];
        let temb = unet.timestep_embedding(t as f32)?;
        let pack_refs: [&[f32]; 4] = [&packs[0], &packs[1], &packs[2], &packs[3]];
        let branches = predict_v_cfg_three_branch(
            unet, &pack_refs, &temb, &enc_alb, &enc_mr, &dino, &cache, &voxels, s8, s8,
        )?;
        batch
            .apply_cfg_step(&sched, &branches.stacked(), t)
            .map_err(|e| format!("ddim step {i}: {e:?}"))?;
        if i == 0 {
            isolated(fails, "DDIM_X1", &batch.sample, need(acts, "ddim_x1")?, 5e-3);
        }
        if i == 7 {
            isolated(fails, "DDIM_X8", &batch.sample, need(acts, "ddim_x8")?, 5e-2);
        }
        if i == 14 {
            isolated(fails, "DDIM_X15", &batch.sample, need(acts, "ddim_x15")?, 5e-2);
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn check(fails: &mut Vec<String>, name: &str, actual: &[f32], json: &str, key: &str, atol: f32) {
    match json_f32s(json, key) {
        Ok(expected) => {
            if let Err(e) = compare(name, actual, &expected, atol) {
                fails.push(e);
            }
        }
        Err(_) => println!("PBR_UNET_{name}_VS_ORACLE skipped (no {key})"),
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn compare(name: &str, actual: &[f32], expected: &[f32], atol: f32) -> Result<f32, String> {
    let n = actual.len().min(expected.len());
    let mut max_abs = 0.0f32;
    for i in 0..n {
        max_abs = max_abs.max((actual[i] - expected[i]).abs());
    }
    println!("PBR_UNET_{name}_VS_ORACLE max_abs={max_abs:.9e} n={n}");
    if max_abs > atol {
        return Err(format!("{name} vs oracle {max_abs}"));
    }
    Ok(max_abs)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn isolated(fails: &mut Vec<String>, name: &str, actual: &[f32], expected: &[f32], atol: f32) {
    if let Err(e) = compare(name, actual, expected, atol) {
        fails.push(e);
    }
}

fn need<'a>(acts: &'a std::collections::HashMap<String, Vec<f32>>, name: &str) -> Result<&'a [f32], String> {
    acts.get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing act {name}"))
}

fn load_acts(path: &str) -> Result<std::collections::HashMap<String, Vec<f32>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(n) = parts.next() else { continue };
        let n: usize = n.parse().map_err(|e| format!("{name} n: {e}"))?;
        let vals: Result<Vec<f32>, _> = parts.map(|s| s.parse()).collect();
        let vals = vals.map_err(|e| format!("{name} parse: {e}"))?;
        if vals.len() != n {
            return Err(format!("{name} len {} vs {n}", vals.len()));
        }
        out.insert(name.to_string(), vals);
    }
    Ok(out)
}

fn json_f32s(json: &str, key: &str) -> Result<Vec<f32>, String> {
    let start = json.find(key).ok_or_else(|| format!("no {key}"))?;
    let rest = &json[start + key.len()..];
    let lb = rest.find('[').ok_or("no [")?;
    let rb = rest[lb..].find(']').ok_or("no ]")?;
    rest[lb + 1..lb + rb]
        .split(',')
        .map(|s| s.trim().parse::<f32>().map_err(|e| e.to_string()))
        .collect()
}
