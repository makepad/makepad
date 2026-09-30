//! Prepare training shards (48 kHz audio, f0, tokens, notes).
//!
//!   sing_prep speech <corpus dir> <out dir> [threads]
//!       Read-speech corpus: every `<speaker>_<chapter>_<utt>.wav` with its
//!       `.normalized.txt` transcript; tokens from the English G2P.
//!   sing_prep vowels <corpus dir> <out dir> [threads]
//!       Sung-technique corpus: files ending `_<a|e|i|o|u>.wav` become
//!       sustained-vowel sung items (notes from the f0); the rest audio-only.
//!
//! Speaker ids: speech speakers are numbered from 0 in sorted order; singers
//! from 1536. The mapping is written to `<out dir>/speakers.tsv`.

use makepad_ai_sing::data::{self, Item, Kind};
use makepad_ai_sing::dsp;
use makepad_ai_sing::phonemes as ph;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const SINGER_BASE: u32 = 1536;
const SHARD: usize = 400;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "__MACOSX" {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map(|x| x == "wav").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn pronounce(word: &str) -> String {
    makepad_ai_speech::g2p::pronounce(word)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: sing_prep speech|vowels <corpus dir> <out dir> [threads]");
        std::process::exit(2);
    }
    let (mode, src, out) = (args[1].as_str(), PathBuf::from(&args[2]), PathBuf::from(&args[3]));
    let threads: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(16);
    std::fs::create_dir_all(&out).unwrap();
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    eprintln!("{} wav files", files.len());

    // Speaker numbering.
    let speaker_of = |p: &Path| -> String {
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        match mode {
            "speech" => stem.split('_').next().unwrap_or("0").to_string(),
            _ => stem.split('_').next().unwrap_or("x").to_string(),
        }
    };
    let mut speakers: Vec<String> = files.iter().map(|p| speaker_of(p)).collect();
    speakers.sort();
    speakers.dedup();
    let base = if mode == "speech" { 0 } else { SINGER_BASE };
    let tsv: String = speakers.iter().enumerate().map(|(i, s)| format!("{}\t{mode}\t{s}\n", base + i as u32)).collect();
    std::fs::write(out.join("speakers.tsv"), tsv).unwrap();

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let shard_no = AtomicUsize::new(0);
    let stats = Mutex::new((0usize, 0f64));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut batch: Vec<Item> = Vec::new();
                let flush = |batch: &mut Vec<Item>| {
                    if batch.is_empty() {
                        return;
                    }
                    let n = shard_no.fetch_add(1, Ordering::Relaxed);
                    data::write_shard(&out.join(format!("{mode}-{n:05}.mksdat")), batch).unwrap();
                    batch.clear();
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= files.len() {
                        break;
                    }
                    let p = &files[i];
                    let Ok(bytes) = std::fs::read(p) else { continue };
                    let Some((x, rate)) = dsp::read_wav(&bytes) else {
                        eprintln!("skip (format) {}", p.display());
                        continue;
                    };
                    let speaker = base + speakers.binary_search(&speaker_of(p)).unwrap() as u32;
                    let item = match mode {
                        "speech" => {
                            let txt = p.with_extension("normalized.txt");
                            let Ok(text) = std::fs::read_to_string(&txt) else { continue };
                            let tokens = data::text_tokens(&text, &pronounce);
                            let (audio, f0, band) = data::prepare_audio(&x, rate);
                            if tokens.len() < 3 || f0.len() < tokens.len() + 10 {
                                continue;
                            }
                            Item { kind: Kind::Speech, speaker, band_hz: band, tokens, notes: Vec::new(), f0, audio }
                        }
                        _ => {
                            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
                            let vowel = match stem.rsplit('_').next() {
                                Some("a") => ph::id("ɑ"),
                                Some("e") => ph::id("ɛ"),
                                Some("i") => ph::id("i"),
                                Some("o") => ph::id("o"),
                                Some("u") => ph::id("u"),
                                _ => None,
                            };
                            let (audio, f0, band) = data::prepare_audio(&x, rate);
                            match vowel {
                                Some(v) => {
                                    let notes = data::notes_from_f0(&f0);
                                    Item { kind: Kind::Sung, speaker, band_hz: band, tokens: data::vowel_tokens(v), notes, f0, audio }
                                }
                                None => Item { kind: Kind::Audio, speaker, band_hz: band, tokens: Vec::new(), notes: Vec::new(), f0, audio },
                            }
                        }
                    };
                    {
                        let mut st = stats.lock().unwrap();
                        st.0 += 1;
                        st.1 += item.audio.len() as f64 / dsp::SR as f64;
                    }
                    batch.push(item);
                    if batch.len() >= SHARD {
                        flush(&mut batch);
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d % 2000 == 0 {
                        eprintln!("{d}/{}", files.len());
                    }
                }
                flush(&mut batch);
            });
        }
    });
    let st = stats.lock().unwrap();
    eprintln!("prepared {} items, {:.1} h", st.0, st.1 / 3600.0);
}
