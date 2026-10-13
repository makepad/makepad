//! Oracle driver for the ggml path. No CUDA-store probes or synchronizers:
//! DeviceGraphSession::execute completes each measured output readback.
use super::*;

fn teacher_forced(pipe: &Yue2Pipeline, dir: &Path, phase: Yue2Phase) {
    let (prefix, tokens, logits) = match phase {
        Yue2Phase::Abc => ("prefix_abc.i32", "abc.i32", "tf_abc_logits.f32"),
        Yue2Phase::Semantic => ("prefix_sem.i32", "semantic.i32", "tf_sem_logits.f32"),
    };
    let prefix = read_i32(&dir.join(prefix));
    let mut ids = read_i32(&dir.join(tokens));
    if phase == Yue2Phase::Semantic { ids.iter_mut().for_each(|id| *id += CODEC_OFFSET); }
    let oracle = read_f32(&dir.join(logits));
    let width = phase.head_rows().1 as usize;
    assert_eq!(oracle.len() % width, 0);
    let rows = oracle.len() / width;
    assert!(rows > 0 && ids.len() >= rows);
    let mut session = pipe.graph_model.as_ref().unwrap().ar_session(&prefix, None, rows, phase).unwrap();
    let (mut worst, mut agree, mut overlap) = (0.0f64, 0usize, 0usize);
    for r in 0..rows {
        let (_, logits, _) = session.logits();
        let refr = &oracle[r * width..(r + 1) * width];
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (&a, &b) in logits.iter().zip(refr) {
            assert!(a.is_finite(), "{phase:?} nonfinite graph logit at row {r}");
            num += ((a - b) as f64).powi(2);
            den += (b as f64).powi(2);
        }
        worst = worst.max((num / den).sqrt());
        let range = match phase { Yue2Phase::Abc => 0..EOD as usize, Yue2Phase::Semantic => 1..width };
        agree += (argmax(&logits[range.clone()]) == argmax(&refr[range.clone()])) as usize;
        overlap += topk_overlap(&logits[range.clone()], &refr[range], 100);
        if r + 1 < rows { session.decode(ids[r]).unwrap(); }
    }
    println!("tf {phase:?} ggml Q8_0 AR: {rows} rows, worst rel_l2 {worst:.3e}, argmax {agree}/{rows}, mean top100 {:.1}", overlap as f64 / rows as f64);
    match phase {
        Yue2Phase::Abc => assert_eq!(agree, rows, "ABC teacher-forced argmax gate"),
        Yue2Phase::Semantic => {
            assert!(worst <= 2.5e-2, "semantic teacher-forced rel_l2 gate: {worst}");
            // Keep the stricter M3b regression gate (the Q8 acceptance
            // floor is 240/256, the overall task requires 245/256).
            assert!(agree * 256 >= 245 * rows, "semantic teacher-forced argmax gate");
        }
    }
}

fn bench(pipe: &Yue2Pipeline, dir: &Path, phase: Yue2Phase) {
    let (file, mut sampling) = match phase {
        Yue2Phase::Abc => ("prefix_abc.i32", Yue2Sampling::ABC),
        Yue2Phase::Semantic => ("prefix_sem.i32", Yue2Sampling::SEMANTIC),
    };
    let prefix = read_i32(&dir.join(file));
    let steps = 200;
    sampling.min_tokens = sampling.min_tokens.max(steps + 3);
    sampling.max_tokens = sampling.max_tokens.max(steps + 3);
    let start = Instant::now();
    let mut session = pipe.graph_model.as_ref().unwrap().ar_session(&prefix, None, sampling.max_tokens, phase).unwrap();
    println!("bench ggml Q8_0 packing/upload/blocked prefill {:.3}s", start.elapsed().as_secs_f64());
    let mut sampler = Yue2Sampler::new(1);
    let mut history = Vec::new();
    let (mut forward, mut sample) = (0.0, 0.0);
    for step in 0..steps + 2 {
        let t = Instant::now();
        let (lo, logits, _) = session.logits();
        let token = sampler.sample(logits, lo, phase, &sampling, &history, step, false);
        let sample_dt = t.elapsed().as_secs_f64();
        history.push(token);
        let t = Instant::now();
        session.decode(token).unwrap();
        if step >= 2 { forward += t.elapsed().as_secs_f64(); sample += sample_dt; }
    }
    println!("bench {phase:?} ggml Q8_0 ctx {}..{}: forward+head+logits download {:.3} ms, host sample {:.3} ms => {:.1} tok/s",
        prefix.len() + 2, prefix.len() + steps + 2, 1e3 * forward / steps as f64,
        1e3 * sample / steps as f64, steps as f64 / (forward + sample));
}

fn flow(pipe: &Yue2Pipeline, dir: &Path, request: &Yue2Request) -> Vec<f32> {
    let prefix = read_i32(&dir.join("prefix_sem.i32"));
    let sem = read_i32(&dir.join("semantic.i32"));
    let t = Instant::now();
    let latents = pipe.synthesize(&prefix, &sem, request.seed, Yue2Pipeline::default_steps(), &mut |_| Ok(())).unwrap();
    println!("ggml flow {} frames {:.3}s (includes setup and host ODE)", sem.len(), t.elapsed().as_secs_f64());
    let rel = compare("synth latents", &latents, &read_f32(&dir.join("latents.f32")));
    assert!(rel <= 5e-2, "synth latents gate: {rel}");
    latents
}

fn nar(pipe: &Yue2Pipeline, dir: &Path, request: &Yue2Request, evals: usize) {
    let mut ar = read_i32(&dir.join("prefix_sem.i32"));
    let sem = read_i32(&dir.join("semantic.i32"));
    let noise = read_f32(&dir.join("nar_noise.f32"));
    let ours = torch_cpu_randn(request.seed, sem.len() * YUE2_LATENT_DIM);
    let rel = compare("nar noise (chunk 0)", &ours[..noise.len()], &noise);
    assert!(rel <= 1e-6, "NAR noise gate: {rel}");
    let frames = noise.len() / YUE2_LATENT_DIM;
    ar.extend(sem[..frames].iter().map(|&c| c + CODEC_OFFSET));
    ar.push(MUSIC_END);
    let t = Instant::now();
    let mut chunk = pipe.graph_model.as_ref().unwrap().nar_chunk(&ar, frames, &[20.0]).unwrap();
    println!("ggml nar Q8_0 prefix/setup {} tokens {:.3}s", ar.len(), t.elapsed().as_secs_f64());
    let t = Instant::now();
    let v0 = chunk.velocity(&noise, 0).unwrap();
    println!("ggml nar first eval with row blocks {:.3}s", t.elapsed().as_secs_f64());
    let rel = compare("nar v0", &v0, &read_f32(&dir.join("nar_v0.f32")));
    assert!(rel <= 1.3e-2, "NAR v0 gate: {rel}");
    let t = Instant::now();
    for _ in 0..evals { chunk.velocity(&noise, 0).unwrap(); }
    println!("ggml nar warm {evals} evals {:.3} ms/eval (state upload/velocity download included)", t.elapsed().as_secs_f64() * 1e3 / evals as f64);
}

pub(super) fn acoustic(pipe: &Yue2Pipeline, vae_dir: &Path, dir: &Path, evals: usize) {
    assert!(evals > 0, "--velocity-evals must be positive");
    let request = load_request(&dir.join("fixture.json"));
    nar(pipe, dir, &request, evals);
    flow(pipe, dir, &request);
    check_vae(vae_dir, dir, None);
}

pub(super) fn check(pipe: &Yue2Pipeline, vae_dir: Option<&Path>, dir: &Path, stage: &str, bench_phase: Yue2Phase) {
    let request = load_request(&dir.join("fixture.json"));
    let all = stage == "all";
    assert!(["all", "tok", "prefill", "greedy", "tf", "sample", "bench", "nar", "flow", "vae", "synth"].contains(&stage),
        "stage {stage} is a CUDA-store diagnostic, not a ggml model gate");
    let prefix = read_i32(&dir.join("prefix_abc.i32"));
    if all || stage == "tok" {
        for (name, text, file) in [
            ("style", request.style.clone(), "tok_style.i32"),
            ("lyrics", request.lyrics.clone(), "tok_lyrics.i32"),
            ("abc", std::fs::read_to_string(dir.join("abc.txt")).unwrap(), "tok_abc.i32"),
        ] {
            let ours = pipe.tokenizer.encode(&text);
            let refr = read_i32(&dir.join(file));
            assert_eq!(ours, refr, "tokenizer {name}");
            println!("tok {name}: identical {} ids", ours.len());
        }
        assert_eq!(makepad_ai_music::yue2::yue2_prefix(&pipe.text_ids(&request), request.cot, None), prefix, "prefix ids");
        assert_eq!(pipe.tokenizer.decode(&read_i32(&dir.join("abc.i32"))), std::fs::read_to_string(dir.join("abc.txt")).unwrap(), "ABC decode");
    }
    if all || stage == "prefill" {
        let t = Instant::now();
        let session = pipe.graph_model.as_ref().unwrap().ar_session(&prefix, None, 1, Yue2Phase::Abc).unwrap();
        let (lo, logits, _) = session.logits();
        println!("ggml prefill {} tokens {:.3}s (includes weight upload/compile)", prefix.len(), t.elapsed().as_secs_f64());
        let oracle = read_f32(&dir.join("abc_prefill_logits.f32"));
        let refr = &oracle[lo as usize..lo as usize + logits.len()];
        let rel = compare("prefill logits", logits, refr);
        assert!(rel <= 4e-3, "prefill rel_l2 gate: {rel}");
        assert_eq!(topk_overlap(&logits[..EOD as usize], &refr[..EOD as usize], 50), 50, "prefill top50 gate");
        println!("prefill top50 50/50");
    }
    if all || stage == "greedy" {
        let refr = read_i32(&dir.join("greedy_abc.i32"));
        let (ids, _, timing) = pipe.generate(&prefix, &Yue2Sampling::GREEDY, request.seed, Yue2Phase::Abc,
            1.0, None, false, &mut |_| Ok(())).unwrap();
        println!("greedy ggml {}/{} ({:.1} tok/s)", prefix_match(&ids, &refr), refr.len(), timing.tokens_per_second());
        assert_eq!(prefix_match(&ids, &refr), refr.len(), "greedy gate");
    }
    if all || stage == "tf" {
        teacher_forced(pipe, dir, Yue2Phase::Abc);
        teacher_forced(pipe, dir, Yue2Phase::Semantic);
    }
    if all || stage == "sample" {
        let plan = pipe.plan(&request, &Yue2Sampling::ABC, &mut |_| Ok(())).unwrap();
        let abc = read_i32(&dir.join("abc.i32"));
        println!("sampled ggml ABC identical prefix {}/{}", prefix_match(&plan.abc_ids, &abc), abc.len());
        let mut oracle_plan = plan;
        oracle_plan.abc_ids = abc;
        oracle_plan.prefix = read_i32(&dir.join("prefix_sem.i32"));
        let refr = read_i32(&dir.join("semantic.i32"));
        let mut sampling = Yue2Sampling::SEMANTIC;
        sampling.max_tokens = sampling.max_tokens.min(refr.len() + 50);
        let sem = pipe.semantic(&request, &oracle_plan, &sampling, &mut |_| Ok(())).unwrap();
        println!("sampled ggml semantic identical prefix {}/{}", prefix_match(&sem.tokens, &refr), refr.len());
    }
    if stage == "bench" { bench(pipe, dir, bench_phase); }
    if all || stage == "nar" { nar(pipe, dir, &request, 1); }
    if all || stage == "flow" || stage == "synth" {
        let latents = flow(pipe, dir, &request);
        if stage == "synth" {
            let decoder = Yue2Vae::load(vae_dir.expect("--vae DIR")).unwrap();
            let audio = decoder.decode(&latents, &mut |_| Ok(())).unwrap();
            let rel = compare("synth audio", &audio, &read_f32(&dir.join("audio.f32")));
            println!("synth audio SNR {:.2} dB", -20.0 * rel.log10());
        }
    }
    if all || stage == "vae" { check_vae(vae_dir.expect("--vae DIR"), dir, None); }
}
