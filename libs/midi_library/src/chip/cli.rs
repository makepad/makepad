//! `midi_library chip ...`: tracker, chiptune and game music as Scores.
//!
//! - `chip info <file>`: what the file holds.
//! - `chip score <file> [out.splash]`: the Score.
//! - `chip text <file>`: the text transcription.
//! - `chip cover <file> <out.wav> [lufs]`: a rendered cover.
//! - `chip ab <file> <dir> [lufs]`: the original (the file's own player,
//!   dry) and the cover side by side, with how alike they sound.
//! - `chip install-hvsc [--accept]`, `chip hvsc [threads]`: the SID
//!   collection installed and catalogued.
//! - `chip search <words>`, `chip get <id> <file>`: one module from The
//!   Mod Archive into the library.
//! - `chip trace <file.sid> [seconds] [n]`, `chip 7z <archive> [name]`,
//!   `chip timing <file>`: what the importer saw.

use crate::chip::import::{self, About};
use crate::{cover, roll};

/// Run `midi_library chip <args>`.
pub fn run(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("install-hvsc") {
        // The SID collection through the pinned installer, its notice first.
        use makepad_model_install::{self as models, InstallMsg};
        let a = &crate::chip::hvsc::ARCHIVE;
        if models::installed(a) && !models::read_provenance(&models::dest_path(a)).is_empty() {
            println!("the SID collection is installed");
            return Ok(());
        }
        if !args.iter().any(|a| a == "--accept") {
            println!("== {} ({:.1} MB)\n{}\n", a.short, a.bytes as f64 / 1e6, a.license_text);
            return Err("read the notice above, then run `midi_library chip install-hvsc --accept`".into());
        }
        let handle = models::start_install(vec![a], Some(models::unix_now()));
        for msg in handle.rx.iter() {
            match msg {
                InstallMsg::Progress { short, done, total } if done == total || (done * 100 / total.max(1)) % 20 == 0 => println!("{short}: {}%", done * 100 / total.max(1)),
                InstallMsg::Done { short } => println!("{short}: installed"),
                InstallMsg::Failed { short, error } => return Err(format!("{short}: {error}")),
                InstallMsg::Cancelled => return Err("cancelled".into()),
                InstallMsg::Finished => break,
                _ => {}
            }
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("search") {
        // One song at a time from an archive's own site.
        let words = args[1..].join(" ");
        let source = &crate::chip::web::MODARCHIVE;
        println!("{}\n", source.terms);
        for h in crate::chip::web::search(source, &words)? {
            println!("{:8} {:4} {:30} {}", h.id, h.format, h.file, h.title);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("zophar") {
        // `chip zophar <words>`: the games; `chip zophar --get <page>`: one.
        if args.get(1).map(String::as_str) == Some("--get") {
            let page = args.get(2).ok_or("chip zophar --get /music/<console>/<game>")?;
            let game = crate::chip::zophar::Game { page: page.clone(), name: page.rsplit('/').next().unwrap_or(page).to_string(), console: String::new(), year: String::new(), developer: String::new() };
            for e in crate::chip::zophar::download(&game)? {
                println!("{} {:5} {:60} {}", e.id, e.format, e.title, e.path);
            }
            return Ok(());
        }
        println!("{}\n", crate::chip::zophar::NOTICE);
        for g in crate::chip::zophar::search(&args[1..].join(" "))? {
            println!("{:60} {:34} {} {}", g.page, g.console, g.year, g.name);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("get") {
        let (id, file) = match &args[1..] {
            [id, file, ..] => (id.clone(), file.clone()),
            _ => return Err("chip get <id> <file> (from chip search)".into()),
        };
        let hit = crate::chip::web::Hit { source: "modarchive", id, title: String::new(), file, format: String::new() };
        let e = crate::chip::web::download(&hit)?;
        println!("{} ({}): {} -- {}\n{}", e.id, e.format, e.title, e.artist, e.tid);
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("hvsc") {
        // Unpack the installed SID collection into its pack and catalogue.
        let threads = args.get(1).and_then(|t| t.parse().ok()).unwrap_or(4);
        let t = std::time::Instant::now();
        let n = crate::chip::hvsc::unpack(threads, &|done, total| eprintln!("  {done}/{total} tunes"))?;
        println!("{n} tunes catalogued in {:.0} s: {}", t.elapsed().as_secs_f64(), crate::chip::hvsc::catalogue_path().display());
        return Ok(());
    }
    let (cmd, file) = match args {
        [cmd, file, ..] => (cmd.as_str(), file.as_str()),
        _ => return Err("usage: midi_library chip info|score|text|cover|ab|trace|7z|timing <file> [out] [lufs]".into()),
    };
    // A file, or a library tune by its id or "Artist - Title".
    // `--song n` (a SID's subtune, an NSF's track) and `--seconds s`.
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<f64>().ok());
    let (bytes, file, about) = if std::path::Path::new(file).is_file() {
        let about = About { song: opt("--song").unwrap_or(0.0) as u16, seconds: opt("--seconds").unwrap_or(0.0), ..About::default() };
        (std::fs::read(file).map_err(|e| format!("{file}: {e}"))?, file.to_string(), about)
    } else {
        let entries = crate::index::load(&crate::index::index_path())?;
        let e = crate::index::find(&entries, file).ok_or_else(|| format!("no file or library tune `{file}`"))?;
        let name = e.path.rsplit_once('#').map_or(e.path.clone(), |(p, _)| p.to_string());
        (crate::index::bytes(e)?, name, import::about_entry(e))
    };
    let file = file.as_str();
    if cmd == "7z" {
        let t = std::time::Instant::now();
        let a = crate::chip::archive::sevenz::Archive::open(&bytes)?;
        println!("{} files in {} folders ({:.2} s to read the header)", a.entries.len(), a.folders(), t.elapsed().as_secs_f64());
        for (coders, files, size) in a.describe() {
            println!("  folder: coders {coders:02x?}, {files} files, {size} bytes");
        }
        let mut total = 0usize;
        let mut sids = 0usize;
        a.for_each(|e, b| {
            total += b.len();
            if e.name.ends_with(".sid") {
                sids += 1;
            }
            if args.get(2).is_some_and(|want| e.name.ends_with(want.as_str())) {
                println!("{} {} bytes: {:?}", e.name, b.len(), String::from_utf8_lossy(&b[..b.len().min(200)]));
            }
        })?;
        println!("{total} bytes unpacked, {sids} .sid, {:.2} s", t.elapsed().as_secs_f64());
        return Ok(());
    }
    if cmd == "trace" {
        // A SID's register writes: the rate, and the first of them.
        let tune = crate::chip::sid::read(&bytes)?;
        println!("{:?}", tune.header);
        let secs: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(10.0);
        let t = crate::chip::sid::machine::run(&tune, tune.header.start_song, secs);
        println!("rate {:.2}/s, {} writes, last at {:.2} s", t.rate, t.writes.len(), t.writes.last().map_or(0.0, |w| w.t));
        for w in t.writes.iter().filter(|w| w.reg % 7 == 4 || w.reg % 7 == 0 || w.reg % 7 == 1).take(args.get(3).and_then(|v| v.parse().ok()).unwrap_or(60)) {
            println!("{:8.4} chip {} reg {:2} = {:02x}", w.t, w.chip, w.reg, w.value);
        }
        return Ok(());
    }
    let t = std::time::Instant::now();
    let imported = import::import(&bytes, file, &about)?;
    let took = t.elapsed().as_secs_f64();
    let a = &imported.arrangement;
    match cmd {
        "info" => {
            println!("{} -- {} | {} | {} bpm | {} bars | {} sections | key {} | {:.2} s to import", a.title, a.artist, imported.format, a.bpm, a.bar_count(), a.sections.len(), a.key, took);
            for p in &a.parts {
                let fx = p.notes.iter().filter(|n| n.fx.is_some()).count();
                println!("  {:10} {:6} {:4} notes ({fx} with fx) pan {:5}  {}", p.name, p.role.name(), p.notes.len(), p.pan, p.about);
            }
            println!("  tempo changes: {}", a.tempo.len());
        }
        "timing" => {
            let doc = makepad_score_live::model::load(&imported.source).map_err(|d| format!("{d:?}"))?.doc;
            println!("arrangement: {} beats; score: {} beats, natural length {:.1} s", a.beats(), doc.arrangement_beats(), makepad_score_live::render::natural_length(&doc));
            let mut t = 0.0;
            let mut beat = 0.0;
            let mut bpm = a.bpm;
            for &(b, next) in &a.tempo {
                t += (b - beat) * 60.0 / bpm;
                beat = b;
                bpm = next;
            }
            t += (a.beats() - beat) * 60.0 / bpm;
            println!("arrangement seconds by its tempo map: {t:.1}");
            let ev = makepad_score_live::render::arrangement_tempo(&doc);
            println!("score tempo events {} (first {:?}); arrangement tempo changes {} (first {:?})", ev.len(), ev.iter().take(6).map(|e| (e.beat, e.bpm)).collect::<Vec<_>>(), a.tempo.len(), a.tempo.iter().take(6).collect::<Vec<_>>());
            println!("score bpm {} arrangement bpm {}", doc.clock.bpm, a.bpm);
        }
        "score" => match args.get(2) {
            Some(out) => std::fs::write(out, &imported.source).map_err(|e| e.to_string())?,
            None => print!("{}", imported.source),
        },
        "text" => print!("{}", roll::to_text(a, &roll::Detail::default())),
        "cover" => {
            let out = args.get(2).ok_or("cover <file> <out.wav> [lufs]")?;
            let lufs: f64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(-14.0);
            import::register(&imported);
            let r = cover::render(&imported.source, lufs)?;
            std::fs::write(out, cover::wav(&r.song)).map_err(|e| e.to_string())?;
            println!("{out}: {:.1} s, {:.1} LUFS, true peak {:.1} dBTP", r.song.mix.seconds(), r.lufs, r.true_peak);
        }
        "ab" => {
            // The original (the file's own replay, dry) and the cover
            // (the Score through the engine), side by side.
            let dir = std::path::PathBuf::from(args.get(2).ok_or("ab <file> <dir> [lufs]")?);
            let lufs: f64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(-14.0);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let stem = std::path::Path::new(file).file_name().and_then(|f| f.to_str()).unwrap_or("song").replace(['.', ' '], "_");
            import::register(&imported);
            let r = cover::render(&imported.source, lufs)?;
            let rate = r.song.mix.rate;
            let (mut ol, mut or) = import::original(&bytes, file, &about, rate)?;
            // The original from where the Score's beat 0 is.
            let skip = ((imported.offset * rate as f64) as usize).min(ol.len());
            ol.drain(..skip);
            or.drain(..skip);
            let like = crate::chip::compare::likeness((&ol, &or), (&r.song.mix.left, &r.song.mix.right), rate);
            std::fs::write(dir.join(format!("{stem}.original.wav")), makepad_music_iter::render::wav(rate, &ol, &or)).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{stem}.cover.wav")), cover::wav(&r.song)).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{stem}.splash")), &r.source).map_err(|e| e.to_string())?;
            println!("{stem}: {} | cover {:.1} s at {:.1} LUFS | original {:.1} s | likeness chroma {:.3} spectrum {:.3} over {} frames", imported.format, r.song.mix.seconds(), r.lufs, ol.len() as f64 / rate as f64, like.chroma, like.spectrum, like.frames);
        }
        _ => return Err(format!("no command `{cmd}`")),
    }
    Ok(())
}
