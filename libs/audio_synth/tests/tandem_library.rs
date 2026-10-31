//! The hybrid synth's own patch library through the Tandem loader
//! (TESTS.md A1-A3): every `.fxp` parses, every parameter element is known,
//! load -> save -> load is lossless, and every patch renders finite audio.
//! The library is an opt-in download (never in the repo): point
//! `TANDEM_PATCH_LIBRARY` at its unpacked data folder (the one holding
//! `patches_factory`); without it the test skips.

use makepad_audio_synth::tandem::{LoadMode, Patch, Tandem};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn library() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("TANDEM_PATCH_LIBRARY")?);
    dir.is_dir().then_some(dir)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fxp")) {
            out.push(p);
        }
    }
}

#[test]
fn every_library_patch_loads_round_trips_and_plays() {
    let Some(root) = library() else {
        eprintln!("tandem_library: TANDEM_PATCH_LIBRARY not set (the opt-in patch download): skipped");
        return;
    };
    let mut files = Vec::new();
    for sub in ["patches_factory", "patches_3rdparty"] {
        walk(&root.join(sub), &mut files);
    }
    assert!(!files.is_empty(), "no .fxp under {}", root.display());
    let mut by_rev: BTreeMap<i32, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut unknown: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut lossy = Vec::new();
    let mut silent = Vec::new();
    let mut bad_audio = Vec::new();
    let render_every = std::env::var("TANDEM_RENDER_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(1usize).max(1);
    let mut engine = Tandem::new(48_000.0);
    for (i, f) in files.iter().enumerate() {
        let bytes = std::fs::read(f).unwrap();
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        let p = match Patch::from_fxp(&bytes, &stem, LoadMode::Patch) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: {e}", f.display()));
                continue;
            }
        };
        *by_rev.entry(p.loaded_revision).or_default() += 1;
        for u in &p.unknown {
            unknown.entry(u.clone()).or_default().push(stem.clone());
        }
        // A3: save and load again, identical state.
        let again = Patch::from_fxp(&p.to_fxp(), &stem, LoadMode::Patch).expect("re-read");
        if format!("{:?}", again.values) != format!("{:?}", p.values) || again.routings != p.routings || again.tables != p.tables || again.stepseq != p.stepseq || format!("{:?}", again.msegs) != format!("{:?}", p.msegs) || again.formulae != p.formulae || again.macros != p.macros || again.porta != p.porta || again.nonparam != p.nonparam {
            let diff = p.values.iter().zip(again.values.iter()).position(|(a, b)| format!("{a:?}") != format!("{b:?}"));
            let what = match diff {
                Some(i) => format!("{}: {:?} -> {:?}", makepad_audio_synth::tandem::catalog::catalog().params[i].name, p.values[i], again.values[i]),
                None => {
                    let fields = [
                        ("routings", again.routings != p.routings),
                        ("tables", again.tables != p.tables),
                        ("stepseq", again.stepseq != p.stepseq),
                        ("msegs", format!("{:?}", again.msegs) != format!("{:?}", p.msegs)),
                        ("formulae", again.formulae != p.formulae),
                        ("macros", again.macros != p.macros),
                        ("porta", again.porta != p.porta),
                        ("nonparam", again.nonparam != p.nonparam),
                    ];
                    let names: Vec<&str> = fields.iter().filter(|f| f.1).map(|f| f.0).collect();
                    let detail = if format!("{:?}", again.msegs) != format!("{:?}", p.msegs) {
                        let (s, l) = (0..2).flat_map(|s| (0..12).map(move |l| (s, l))).find(|(s, l)| again.msegs[*s][*l] != p.msegs[*s][*l]).unwrap();
                        format!(" mseg {s}/{l}: {:?} vs {:?}", p.msegs[s][l].as_ref().map(|m| (m.segments.len(), m.loop_start_point, m.legacy_loop_start, m.endpoint_mode)), again.msegs[s][l].as_ref().map(|m| (m.segments.len(), m.loop_start_point, m.legacy_loop_start, m.endpoint_mode)))
                    } else if again.routings != p.routings {
                        format!(" routings {} vs {}", p.routings.len(), again.routings.len())
                    } else {
                        String::new()
                    };
                    format!("{names:?}{detail}")
                }
            };
            lossy.push(format!("{} ({what})", f.display()));
        }
        if i % render_every == 0 {
            engine.load(&p);
            engine.all_notes_off();
            let mut l = vec![0.0f32; 24_000];
            let mut r = vec![0.0f32; 24_000];
            engine.note_on(0, 60, 100);
            engine.render(&mut l, &mut r);
            engine.note_off(0, 60, 64);
            let mut l2 = vec![0.0f32; 4_800];
            let mut r2 = vec![0.0f32; 4_800];
            engine.render(&mut l2, &mut r2);
            let all = l.iter().chain(r.iter()).chain(l2.iter()).chain(r2.iter());
            let mut peak = 0.0f32;
            let mut finite = true;
            for v in all {
                finite &= v.is_finite();
                peak = peak.max(v.abs());
            }
            if !finite || peak > 16.0 {
                bad_audio.push(format!("{} (peak {peak})", f.display()));
            } else if peak < 1e-5 {
                silent.push(stem.clone());
            }
        }
    }
    eprintln!("tandem_library: {} patches; by revision {:?}", files.len(), by_rev);
    eprintln!("tandem_library: {} silent at note 60 (some patches need input, a key range or a mod)", silent.len());
    if !silent.is_empty() {
        eprintln!("  silent: {}", silent.iter().take(40).cloned().collect::<Vec<_>>().join(", "));
    }
    // A2: parameter elements the catalogue does not know (reported; the spec lane is asked about each).
    for (name, files) in &unknown {
        eprintln!("tandem_library: unknown element `{name}` in {} patch(es): {}", files.len(), files.iter().take(5).cloned().collect::<Vec<_>>().join(", "));
    }
    assert!(failures.is_empty(), "{} failed to load:\n{}", failures.len(), failures.join("\n"));
    assert!(lossy.is_empty(), "{} not lossless:\n{}", lossy.len(), lossy.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
    assert!(bad_audio.is_empty(), "{} rendered non-finite or > +24 dBFS:\n{}", bad_audio.len(), bad_audio.join("\n"));
}
