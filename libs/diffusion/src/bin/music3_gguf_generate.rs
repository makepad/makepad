//! Mac-only Music3 GGUF generate: official ModularPipeline graph on the
//! audio.cpp Q4 pack. Does not use the CUDA safetensors path.
//!
//! `--full` is the default (AR CFG/top-k → RVQ 7 residuals → cond → DiT Euler
//! → vocoder). There is no dummy vocoder-only path.
//!
//!   music3-gguf-generate --weights /Users/dev/metal-probe/music3 \
//!       --full --frames 8 --steps 4 --lm-layers 8 --seed 7 \
//!       --out /Users/dev/makepad/music3_q4.wav

use makepad_diffusion::music3::{
    assemble_prompt, load_tokenizer, music3_latent_len, music3_max_frames, tokenize_cfg_pair,
    MUSIC3_AUDIO_CHANNELS, MUSIC3_DIT_COND, MUSIC3_DIT_IN_CHANNELS, MUSIC3_FLOW_STEPS,
    MUSIC3_FRAME_RATE,
    MUSIC3_LM_LAYERS, MUSIC3_PINE_LYRICS, MUSIC3_PINE_PROMPT, MUSIC3_SAMPLE_RATE,
};
use makepad_diffusion::music3_gguf::FiniteStats;
use makepad_diffusion::music3_gguf_gen::{
    gguf_dit_forward_from, gguf_dit_from_cond, gguf_dit_from_cond_x0, gguf_dit_probe, gguf_generate,
};
use makepad_diffusion::music3_quant::Music3GgufPack;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

fn arg_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn read_u32_file(path: &std::path::Path) -> Vec<u32> {
    let bytes = std::fs::read(path).unwrap_or_else(|err| {
        eprintln!("{}: {err}", path.display());
        std::process::exit(1);
    });
    if bytes.len() % 4 != 0 {
        eprintln!("{}: {} bytes not multiple of 4", path.display(), bytes.len());
        std::process::exit(1);
    }
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn read_f32_file(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|err| {
        eprintln!("{}: {err}", path.display());
        std::process::exit(1);
    });
    if bytes.len() % 4 != 0 {
        eprintln!("{}: {} bytes not multiple of 4", path.display(), bytes.len());
        std::process::exit(1);
    }
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if arg_flag(&args, "--help") || arg_flag(&args, "-h") {
        eprintln!(
            "usage: music3-gguf-generate [--full] [--weights DIR] [--out WAV]\n\
             \t[--frames N] [--seconds S] [--steps N] [--lm-layers N] [--seed N]\n\
             \t[--prompt TEXT] [--lyrics TEXT] [--dit-probe]\n\
             --full is the default official ModularPipeline path.\n\
             --dit-probe dumps time-embed layouts + one dummy DiT forward (no AR)."
        );
        return;
    }
    // --full is accepted and is the default (AR + RVQ + cond + DiT + vocoder).
    let _ = arg_flag(&args, "--full");
    let vocoder_noise = arg_flag(&args, "--vocoder-noise");
    let dit_probe = arg_flag(&args, "--dit-probe");
    let latents_f32 = args
        .windows(2)
        .find(|w| w[0] == "--latents-f32")
        .map(|w| PathBuf::from(&w[1]));
    let cond_f32 = args
        .windows(2)
        .find(|w| w[0] == "--cond-f32")
        .map(|w| PathBuf::from(&w[1]));
    let x0_f32 = args
        .windows(2)
        .find(|w| w[0] == "--x0-f32")
        .map(|w| PathBuf::from(&w[1]));
    let vref_f32 = args
        .windows(2)
        .find(|w| w[0] == "--vref-f32")
        .map(|w| PathBuf::from(&w[1]));
    let semantic_u32 = args
        .windows(2)
        .find(|w| w[0] == "--semantic-u32")
        .map(|w| PathBuf::from(&w[1]));
    let rvq_u32 = args
        .windows(2)
        .find(|w| w[0] == "--rvq-u32")
        .map(|w| PathBuf::from(&w[1]));
    let cond_ref = args
        .windows(2)
        .find(|w| w[0] == "--cond-ref")
        .map(|w| PathBuf::from(&w[1]));
    let weights = args
        .windows(2)
        .find(|w| w[0] == "--weights")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("local/models/music3"));
    let out_wav = args
        .windows(2)
        .find(|w| w[0] == "--out")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("/tmp/music3_gguf.wav"));
    let seconds = args
        .windows(2)
        .find(|w| w[0] == "--seconds")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(5.0f64);
    let steps = args
        .windows(2)
        .find(|w| w[0] == "--steps")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(MUSIC3_FLOW_STEPS)
        .max(1);
    let seed = args
        .windows(2)
        .find(|w| w[0] == "--seed")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(7u64);
    let lm_layers = args
        .windows(2)
        .find(|w| w[0] == "--lm-layers")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(MUSIC3_LM_LAYERS);
    let max_frames = args
        .windows(2)
        .find(|w| w[0] == "--frames")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or_else(|| music3_max_frames(seconds));
    let caption = args
        .windows(2)
        .find(|w| w[0] == "--prompt")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| MUSIC3_PINE_PROMPT.to_string());
    let lyrics = args
        .windows(2)
        .find(|w| w[0] == "--lyrics")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| MUSIC3_PINE_LYRICS.to_string());

    println!(
        "pack {} mode=full seconds={seconds} steps={steps} seed={seed} lm_layers={lm_layers} frames={max_frames} out={}",
        weights.display(),
        out_wav.display()
    );
    let t0 = Instant::now();
    let pack = Music3GgufPack::open(&weights).unwrap_or_else(|err| {
        eprintln!("open: {err}");
        std::process::exit(1);
    });
    println!(
        "open {:.2}s vocoder={} rvq={} lm={} dit={} cond={} lm_path={}",
        t0.elapsed().as_secs_f32(),
        pack.vocoder.tensor_count(),
        pack.rvq.tensor_count(),
        pack.language_model.tensor_count(),
        pack.transformer.tensor_count(),
        pack.condition.tensor_count(),
        pack.paths.language_model.display()
    );

    if let (Some(cond_path), Some(x0_path)) = (cond_f32.as_ref(), x0_f32.as_ref()) {
        if vref_f32.is_none() {
            let cond = read_f32_file(cond_path);
            let x0 = read_f32_file(x0_path);
            if cond.len() % MUSIC3_DIT_COND != 0 {
                eprintln!("cond-f32: {} not multiple of {MUSIC3_DIT_COND}", cond.len());
                std::process::exit(1);
            }
            let tokens = cond.len() / MUSIC3_DIT_COND;
            if x0.len() != MUSIC3_DIT_IN_CHANNELS * tokens {
                eprintln!(
                    "x0-f32: {} floats, expected {}",
                    x0.len(),
                    MUSIC3_DIT_IN_CHANNELS * tokens
                );
                std::process::exit(1);
            }
            println!(
                "euler-from-x0 tokens={tokens} steps={steps} x0 {} cond {}",
                FiniteStats::of(&x0),
                FiniteStats::of(&cond)
            );
            let latents = gguf_dit_from_cond_x0(
                &pack,
                &cond,
                Some(&x0),
                tokens,
                steps,
                seed,
                &mut |stage, k, n| {
                    if k == 0 || k == n || (n > 0 && k % 5 == 0) {
                        println!("  {stage} {k}/{n}");
                    }
                },
            )
            .unwrap_or_else(|err| {
                eprintln!("dit: {err}");
                std::process::exit(1);
            });
            println!("latents {}", FiniteStats::of(&latents));
            let voc = pack.load_vocoder().unwrap_or_else(|err| {
                eprintln!("vocoder: {err}");
                std::process::exit(1);
            });
            let audio = voc.decode(&latents, tokens).unwrap_or_else(|err| {
                eprintln!("decode: {err}");
                std::process::exit(1);
            });
            let samples = audio.len() / MUSIC3_AUDIO_CHANNELS;
            write_wav_i16(&out_wav, &audio, samples).unwrap_or_else(|err| {
                eprintln!("wav: {err}");
                std::process::exit(1);
            });
            println!("wav {}", out_wav.display());
            println!("ok");
            return;
        }
        let cond = read_f32_file(cond_path);
        let x0 = read_f32_file(x0_path);
        if cond.len() % MUSIC3_DIT_COND != 0 {
            eprintln!("cond-f32: {} not multiple of {MUSIC3_DIT_COND}", cond.len());
            std::process::exit(1);
        }
        let tokens = cond.len() / MUSIC3_DIT_COND;
        if x0.len() != MUSIC3_DIT_IN_CHANNELS * tokens {
            eprintln!(
                "x0-f32: {} floats, expected {}",
                x0.len(),
                MUSIC3_DIT_IN_CHANNELS * tokens
            );
            std::process::exit(1);
        }
        println!(
            "dit-step0 tokens={tokens} x0 {} cond {}",
            FiniteStats::of(&x0),
            FiniteStats::of(&cond)
        );
        let v = gguf_dit_forward_from(&pack, &x0, &cond, tokens, 0.0).unwrap_or_else(|err| {
            eprintln!("dit-forward: {err}");
            std::process::exit(1);
        });
        println!("v {}", FiniteStats::of(&v));
        if let Some(pref) = vref_f32.as_ref() {
            let pref = read_f32_file(pref);
            if pref.len() == v.len() {
                let mut max_d = 0f32;
                let mut sum = 0f64;
                let mut n = 0f64;
                let mut dot = 0f64;
                let mut na = 0f64;
                let mut nb = 0f64;
                for (a, b) in v.iter().zip(pref.iter()) {
                    let d = (*a - *b).abs();
                    if d > max_d {
                        max_d = d;
                    }
                    sum += d as f64;
                    n += 1.0;
                    dot += (*a as f64) * (*b as f64);
                    na += (*a as f64) * (*a as f64);
                    nb += (*b as f64) * (*b as f64);
                }
                let cos = dot / (na.sqrt() * nb.sqrt() + 1e-12);
                println!(
                    "v vs official maxabs={max_d:.4} meanabs={:.4} cos={cos:.4}",
                    sum / n.max(1.0)
                );
            } else {
                eprintln!("vref len {} != v {}", pref.len(), v.len());
            }
        }
        println!("ok");
        return;
    }

    if let Some(path) = cond_f32 {
        let cond = read_f32_file(&path);
        if cond.len() % MUSIC3_DIT_COND != 0 {
            eprintln!("cond-f32: {} not multiple of {MUSIC3_DIT_COND}", cond.len());
            std::process::exit(1);
        }
        let tokens = cond.len() / MUSIC3_DIT_COND;
        println!("cond-f32 {} tokens={tokens} {}", path.display(), FiniteStats::of(&cond));
        let latents = gguf_dit_from_cond(&pack, &cond, tokens, steps, seed, &mut |stage, k, n| {
            if k == 0 || k == n || (n > 0 && k % 5 == 0) {
                println!("  {stage} {k}/{n}");
            }
        })
        .unwrap_or_else(|err| {
            eprintln!("dit: {err}");
            std::process::exit(1);
        });
        println!("latents {}", FiniteStats::of(&latents));
        let voc = pack.load_vocoder().unwrap_or_else(|err| {
            eprintln!("vocoder: {err}");
            std::process::exit(1);
        });
        let audio = voc.decode(&latents, tokens).unwrap_or_else(|err| {
            eprintln!("decode: {err}");
            std::process::exit(1);
        });
        let samples = audio.len() / MUSIC3_AUDIO_CHANNELS;
        write_wav_i16(&out_wav, &audio, samples).unwrap_or_else(|err| {
            eprintln!("wav: {err}");
            std::process::exit(1);
        });
        println!("wav {}", out_wav.display());
        println!("ok");
        return;
    }

    if let Some(path) = latents_f32 {
        let bytes = std::fs::read(&path).unwrap_or_else(|err| {
            eprintln!("latents-f32: {err}");
            std::process::exit(1);
        });
        if bytes.len() % 4 != 0 {
            eprintln!("latents-f32: {} bytes not multiple of 4", bytes.len());
            std::process::exit(1);
        }
        let latents: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        if latents.len() % MUSIC3_DIT_IN_CHANNELS != 0 {
            eprintln!(
                "latents-f32: {} floats not divisible by {}",
                latents.len(),
                MUSIC3_DIT_IN_CHANNELS
            );
            std::process::exit(1);
        }
        let tokens = latents.len() / MUSIC3_DIT_IN_CHANNELS;
        println!(
            "latents-f32 {} tokens={tokens} {}",
            path.display(),
            FiniteStats::of(&latents)
        );
        let voc = pack.load_vocoder().unwrap_or_else(|err| {
            eprintln!("vocoder: {err}");
            std::process::exit(1);
        });
        let audio = voc.decode(&latents, tokens).unwrap_or_else(|err| {
            eprintln!("decode: {err}");
            std::process::exit(1);
        });
        let samples = audio.len() / MUSIC3_AUDIO_CHANNELS;
        println!(
            "audio {} stereo samples ({:.2}s) {}",
            samples,
            samples as f64 / MUSIC3_SAMPLE_RATE as f64,
            FiniteStats::of(&audio)
        );
        write_wav_i16(&out_wav, &audio, samples).unwrap_or_else(|err| {
            eprintln!("wav: {err}");
            std::process::exit(1);
        });
        println!("wav {}", out_wav.display());
        println!("ok");
        return;
    }

    if dit_probe {
        let probe_tokens = args
            .windows(2)
            .find(|w| w[0] == "--frames")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(8usize);
        gguf_dit_probe(&pack, probe_tokens.max(2), seed).unwrap_or_else(|err| {
            eprintln!("dit-probe: {err}");
            std::process::exit(1);
        });
        println!("ok");
        return;
    }

    if vocoder_noise {
        let tokens = music3_latent_len(max_frames);
        let mut rng = seed ^ 0xD17E_E001;
        let mut latents = vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens];
        for v in &mut latents {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let u = (rng >> 33) as f32 / (1u32 << 31) as f32;
            *v = (-2.0 * u.max(1e-12).ln()).sqrt() * if rng & 1 == 0 { 1.0 } else { -1.0 };
        }
        println!("vocoder-noise tokens={tokens} {}", FiniteStats::of(&latents));
        let voc = pack.load_vocoder().unwrap_or_else(|err| {
            eprintln!("vocoder: {err}");
            std::process::exit(1);
        });
        let audio = voc.decode(&latents, tokens).unwrap_or_else(|err| {
            eprintln!("decode: {err}");
            std::process::exit(1);
        });
        let samples = audio.len() / MUSIC3_AUDIO_CHANNELS;
        println!(
            "audio {} stereo samples ({:.2}s) {}",
            samples,
            samples as f64 / MUSIC3_SAMPLE_RATE as f64,
            FiniteStats::of(&audio)
        );
        write_wav_i16(&out_wav, &audio, samples).unwrap_or_else(|err| {
            eprintln!("wav: {err}");
            std::process::exit(1);
        });
        println!("wav {}", out_wav.display());
        println!("ok");
        return;
    }

    let tokenizer = load_tokenizer(&pack.paths.tokenizer).unwrap_or_else(|err| {
        eprintln!("tokenizer: {err}");
        std::process::exit(1);
    });
    let assembled = assemble_prompt(&caption, &lyrics);
    let pairs = tokenize_cfg_pair(&tokenizer, &caption, &lyrics).unwrap_or_else(|err| {
        eprintln!("tokenize: {err}");
        std::process::exit(1);
    });
    let cond_ids: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    let uncond_ids: Vec<u32> = pairs.iter().map(|p| p[1]).collect();
    println!("prompt {} chars tokens={}", assembled.len(), cond_ids.len());

    let t1 = Instant::now();
    let mut last_stage = String::new();
    let force_sem = semantic_u32.as_ref().map(|p| read_u32_file(p));
    let force_rvq = rvq_u32.as_ref().map(|p| read_u32_file(p));
    let cond_ref = cond_ref.as_ref().map(|p| read_f32_file(p));
    if let Some(s) = force_sem.as_ref() {
        println!("force-semantic n={}", s.len());
    }
    if let Some(s) = force_rvq.as_ref() {
        println!("force-rvq n={}", s.len());
    }
    let audio = gguf_generate(
        &pack,
        &cond_ids,
        &uncond_ids,
        max_frames,
        steps,
        seed,
        lm_layers,
        force_sem.as_deref(),
        force_rvq.as_deref(),
        cond_ref.as_deref(),
        &mut |stage, k, n| {
            if stage != last_stage || k == 0 || k == n || (n > 0 && k % 5 == 0) {
                println!("  {stage} {k}/{n}");
                let _ = std::io::stdout().flush();
                last_stage = stage.to_string();
            }
        },
    )
    .unwrap_or_else(|err| {
        eprintln!("generate: {err}");
        std::process::exit(1);
    });
    let samples = audio.len() / MUSIC3_AUDIO_CHANNELS;
    println!(
        "audio {:.2}s {} stereo samples ({:.2}s @ {MUSIC3_SAMPLE_RATE}, ~{} frames @ {MUSIC3_FRAME_RATE} fps) {}",
        t1.elapsed().as_secs_f32(),
        samples,
        samples as f64 / MUSIC3_SAMPLE_RATE as f64,
        (samples as f64 * MUSIC3_FRAME_RATE / MUSIC3_SAMPLE_RATE as f64) as usize,
        FiniteStats::of(&audio)
    );
    write_wav_i16(&out_wav, &audio, samples).unwrap_or_else(|err| {
        eprintln!("wav: {err}");
        std::process::exit(1);
    });
    println!("wav {}", out_wav.display());
    println!("ok");
}

fn write_wav_i16(path: &std::path::Path, planar: &[f32], samples: usize) -> Result<(), String> {
    if planar.len() != MUSIC3_AUDIO_CHANNELS * samples {
        return Err(format!(
            "wav planar {} expected {}",
            planar.len(),
            MUSIC3_AUDIO_CHANNELS * samples
        ));
    }
    let mut pcm = Vec::with_capacity(samples * MUSIC3_AUDIO_CHANNELS * 2);
    for t in 0..samples {
        for ch in 0..MUSIC3_AUDIO_CHANNELS {
            let v = planar[ch * samples + t].clamp(-1.0, 1.0);
            pcm.extend_from_slice(&((v * 32767.0).round() as i16).to_le_bytes());
        }
    }
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    let data_len = pcm.len() as u32;
    let sr = MUSIC3_SAMPLE_RATE as u32;
    let ch = MUSIC3_AUDIO_CHANNELS as u16;
    let byte_rate = sr * ch as u32 * 2;
    file.write_all(b"RIFF").map_err(|e| e.to_string())?;
    file.write_all(&(36 + data_len).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    file.write_all(&16u32.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&ch.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&sr.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&byte_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&(ch * 2).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&16u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"data").map_err(|e| e.to_string())?;
    file.write_all(&data_len.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&pcm).map_err(|e| e.to_string())?;
    Ok(())
}
