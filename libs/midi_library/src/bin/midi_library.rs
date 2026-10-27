//! The MIDI song library, headless (the songs are their owners' works;
//! see `status` for the notice and licences).
//!
//!     midi_library status
//!         what is installed, the sizes and the licences
//!     midi_library install --accept [--genres] [--tags]
//!         download what is missing (pinned URL, size and SHA-256), accepting
//!         the licences `status` shows. `--genres` (tagtraum CD2) and `--tags`
//!         (Last.fm) add the research-only extras; a plain install never
//!         fetches them
//!     midi_library index [--threads 4]
//!         read every song into the catalogue (index.tsv beside the files)
//!     midi_library search <words...> [--n 20]
//!         e.g. `80s synthpop`, `eighties new wave quality:60`, `artist:abba`
//!     midi_library import <id | "Artist - Title" | file.mid> <out.splash> [--gm] [--presets] [--kit gm|gm-room|gm-power|gm-electronic|gm-analog|gm-jazz|gm-brush|analog_80s|analog_hybrid|studio|modelled]
//!     midi_library cover <id | "Artist - Title">... (or file.mid ...) [--out <dir>] [--gm] [--kit ...] [--lufs -11] [--mechanical]
//!         import, arrange with the music model (unless --mechanical), render through the
//!         engine, write <slug>.wav, .splash and .card.tsv (and <slug>_mechanical.splash)
//!     midi_library covers <words...> [--n 5] [--out <dir>] [--lufs -11]
//!         the best match of each of the first n artists for the query
//!     midi_library refs <id | "Artist - Title">...
//!         write reference excerpts for the music AI
//!     midi_library text <id | "Artist - Title" | words | file.mid | song.splash> [--bars 1-16] [--parts bass,drums] [--detail full|chords|drums|melody|bass] [--all]
//!         the text transcription (`midi-text` is the same); `--all` writes repeats out
//!     midi_library mcp
//!         the AI tools (midi_lookup, midi_transcribe, score_text) as an MCP server on stdio
//!     midi_library score <song.txt> <out.splash>
//!         a text transcription as a Score

use makepad_midi_library::gm::Voice;
use makepad_midi_library::import::{About, Options};
use makepad_midi_library::index::{self, Entry};
use makepad_midi_library::{cover, dataset, import, refs, roll};
use makepad_model_install::{self as models, InstallMsg};
use std::path::{Path, PathBuf};

fn main() {
    run(std::env::args().skip(1).collect());
}

/// The commands (`midi-text` runs `text`).
pub fn run(args: Vec<String>) {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has = |name: &str| args.iter().any(|a| a == name);
    // Positional words: everything that is not a flag or a flag's value.
    let mut words: Vec<String> = Vec::new();
    let mut skip = false;
    for a in args.iter().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = !matches!(a.as_str(), "--gm" | "--accept" | "--all" | "--mechanical" | "--presets" | "--genres" | "--tags");
            continue;
        }
        words.push(a.clone());
    }
    let result = match args.first().map(String::as_str) {
        Some("status") => status(),
        Some("install") => install(has("--accept"), has("--genres"), has("--tags")),
        Some("index") => build_index(flag("--threads").and_then(|t| t.parse().ok()).unwrap_or(4)),
        Some("search") => search(&words.join(" "), flag("--n").and_then(|n| n.parse().ok()).unwrap_or(20)),
        Some("import") if words.len() == 2 => import_one(&words[0], Path::new(&words[1]), &options(&flag, has("--gm"), has("--presets"))),
        Some("cover") if !words.is_empty() => covers(&words, &out_dir(&flag), &options(&flag, has("--gm"), has("--presets")), lufs(&flag), !has("--mechanical")),
        Some("covers") => pick(&words.join(" "), flag("--n").and_then(|n| n.parse().ok()).unwrap_or(5)).and_then(|ids| covers(&ids, &out_dir(&flag), &options(&flag, has("--gm"), has("--presets")), lufs(&flag), !has("--mechanical"))),
        Some("refs") if !words.is_empty() => references(&words),
        Some("chip") => makepad_midi_library::chip::cli::run(&args[1..]),
        Some("mcp") => {
            makepad_midi_library::tools::serve_mcp();
            Ok(())
        }
        Some("text") if !words.is_empty() => text(&words.join(" "), &flag, has("--all")),
        Some("score") if words.len() == 2 => text_to_score(Path::new(&words[0]), Path::new(&words[1])),
        _ => {
            eprintln!("midi_library status | install --accept | index | search <words> | import <id> <out.splash> | cover <id>... | covers <words> [--n 5] | refs <id>...");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("midi_library: {e}");
        std::process::exit(1);
    }
}

fn lufs(flag: &dyn Fn(&str) -> Option<String>) -> f64 {
    flag("--lufs").and_then(|v| v.parse().ok()).unwrap_or(-11.0)
}

fn out_dir(flag: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    flag("--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../../local/music-iter/covers"))
}

fn options(flag: &dyn Fn(&str) -> Option<String>, gm_only: bool, presets: bool) -> Options {
    let kit = flag("--kit").map(|k| match k.as_str() {
        "modelled" => Voice::ModelledKit,
        "gm" => Voice::GmKit(0),
        "gm-room" => Voice::GmKit(8),
        "gm-power" => Voice::GmKit(16),
        "gm-electronic" => Voice::GmKit(24),
        "gm-analog" => Voice::GmKit(25),
        "gm-jazz" => Voice::GmKit(32),
        "gm-brush" => Voice::GmKit(40),
        "analog_80s" | "tr808" => Voice::Kit("analog_80s"),
        "analog_hybrid" | "tr909" => Voice::Kit("analog_hybrid"),
        "lofi" => Voice::Kit("lofi"),
        "chip" => Voice::Kit("chip"),
        _ => Voice::Kit("studio"),
    });
    Options { gm_only, presets, kit, ..Options::default() }
}

fn status() -> Result<(), String> {
    println!("{}\n", dataset::NOTICE);
    for file in dataset::FILES.iter().chain(dataset::RESEARCH.iter()) {
        let optional = if dataset::RESEARCH.iter().any(|r| std::ptr::eq(*r, *file)) { " (opt-in: install --accept --genres/--tags)" } else { "" };
        let state = if models::installed(file) { "installed" } else { "missing" };
        println!("{:<28} {:>9.1} MB  {state:<9}  {}", file.short, file.bytes as f64 / 1e6, dataset::path(file).display());
        println!("    {}\n    licence: {}{optional}\n", file.origin, file.license);
    }
    let missing = dataset::missing();
    println!("missing: {} file(s), {:.1} MB", missing.len(), dataset::bytes(&missing) as f64 / 1e6);
    let index = index::index_path();
    println!("index: {}", if index.is_file() { index.display().to_string() } else { "not built (midi_library index)".into() });
    Ok(())
}

fn install(accept: bool, genres: bool, tags: bool) -> Result<(), String> {
    // The plain set, plus the research-only extras the person named.
    let mut wanted: Vec<&'static makepad_model_install::Installable> = dataset::FILES.to_vec();
    if genres {
        wanted.push(&dataset::GENRES);
    }
    if tags {
        wanted.push(&dataset::TAGS);
    }
    // Missing, or on disk without the installer's record (put there by
    // hand): the installer checks it against its pin and records it.
    let missing: Vec<_> = wanted.iter().copied().filter(|f| !models::installed(f) || models::read_provenance(&dataset::path(f)).is_empty()).collect();
    if missing.is_empty() {
        println!("everything is installed");
        return Ok(());
    }
    if !accept {
        for file in &missing {
            println!("== {} ({:.1} MB)\n{}\n", file.short, file.bytes as f64 / 1e6, file.license_text);
        }
        return Err(format!("{} file(s), {:.1} MB: read the licences above, then run `midi_library install --accept`", missing.len(), dataset::bytes(&missing) as f64 / 1e6));
    }
    let handle = models::start_install(missing, Some(models::unix_now()));
    let mut failed = Vec::new();
    for msg in handle.rx.iter() {
        match msg {
            InstallMsg::Progress { short, done, total } => {
                if done == total || (done * 100 / total.max(1)) % 10 == 0 {
                    println!("{short}: {}%", done * 100 / total.max(1));
                }
            }
            InstallMsg::Done { short } => println!("{short}: installed"),
            InstallMsg::Failed { short, error } => {
                println!("{short}: FAILED {error}");
                failed.push(short);
            }
            InstallMsg::Cancelled => return Err("cancelled".into()),
            InstallMsg::Finished => break,
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("not installed: {}", failed.join(", "))) }
}

fn build_index(threads: usize) -> Result<(), String> {
    let started = std::time::Instant::now();
    let entries = index::build(threads, &|done, total| println!("read {done}/{total}"))?;
    index::write(&entries, &index::index_path())?;
    let with_year = entries.iter().filter(|e| e.year.is_some()).count();
    let with_tags = entries.iter().filter(|e| !e.tags.is_empty()).count();
    let tagged_genre = entries.iter().filter(|e| e.genres.iter().all(|g| !g.starts_with('~'))).count();
    println!(
        "{} songs indexed in {:.0} s: {with_year} with a year, {tagged_genre} with an MSD genre, {with_tags} with Last.fm tags -> {}",
        entries.len(),
        started.elapsed().as_secs_f64(),
        index::index_path().display()
    );
    Ok(())
}

fn row(e: &Entry, score: f32) -> String {
    let f = &e.features;
    let tags: Vec<&str> = e.tags.iter().take(5).map(|t| t.0.as_str()).collect();
    format!(
        "{}  {:<48} {:>3}  {:<16} {:>5} bpm  {:<9} {:>4}s  q{:<3} {:<30} [{}] {:.2}",
        e.id,
        e.label().chars().take(48).collect::<String>(),
        e.decade().map_or("-".into(), |d| format!("{}s", d % 100)),
        e.genres.join(",").chars().take(16).collect::<String>(),
        f.bpm,
        f.key,
        f.seconds,
        f.quality,
        f.roles.iter().map(|r| r.name()).collect::<Vec<_>>().join(","),
        tags.join(","),
        score
    )
}

fn search(query: &str, n: usize) -> Result<(), String> {
    let entries = index::load(&index::index_path())?;
    let hits = index::search(&entries, query, n);
    for (e, score) in &hits {
        println!("{}", row(e, *score));
    }
    println!("{} match(es) of {} songs", hits.len(), entries.len());
    Ok(())
}

/// The ids of the best version of the first `n` artists for `query`.
fn pick(query: &str, n: usize) -> Result<Vec<String>, String> {
    let entries = index::load(&index::index_path())?;
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<String> = index::search(&entries, &format!("{query} quality:60"), 200)
        .into_iter()
        .filter(|(e, _)| e.features.melody && e.features.seconds >= 120.0 && seen.insert(makepad_midi_library::meta::norm(&e.artist)))
        .take(n)
        .map(|(e, _)| e.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(format!("nothing matches `{query}`"));
    }
    Ok(ids)
}

fn about(e: &Entry) -> About {
    About { title: e.title.clone(), artist: e.artist.clone(), year: e.year, id: e.id.clone(), path: e.path.clone(), genre: e.genres.join(","), tags: e.tags.clone() }
}

fn resolve(names: &[String]) -> Result<Vec<(Entry, Vec<u8>)>, String> {
    let entries = index::load(&index::index_path())?;
    let mut found = Vec::new();
    for name in names {
        found.push(index::find(&entries, name).cloned().ok_or_else(|| format!("no song `{name}` (an id from search, or \"Artist - Title\")"))?);
    }
    if let Some(e) = found.iter().find(|e| e.format != "mid") {
        return Err(format!("{} is {} music: `midi_library chip cover {}`", e.label(), e.format, e.id));
    }
    let paths: Vec<&str> = found.iter().map(|e| e.path.as_str()).collect();
    let bytes = index::midi(&paths)?;
    found.into_iter().zip(bytes).map(|(e, b)| b.map(|b| (e.clone(), b)).ok_or_else(|| format!("{} is not in the archive", e.path))).collect()
}

/// A song by its library name, or any MIDI file by its path (named
/// after the file, no metadata).
fn resolve_one(name: &str) -> Result<(Entry, Vec<u8>), String> {
    let path = Path::new(name);
    if path.is_file() && name.to_lowercase().ends_with(".mid") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let title = path.file_stem().map_or("Song".into(), |s| s.to_string_lossy().to_string());
        let song = makepad_midi_library::analyse::read(&bytes)?;
        let entry = Entry {
            id: makepad_midi_library::md5::hex(&bytes)[..8].to_string(),
            artist: "Unknown".into(),
            title,
            path: name.to_string(),
            md5: String::new(),
            tid: String::new(),
            matched: 0.0,
            year: None,
            genres: Vec::new(),
            tags: Vec::new(),
            features: makepad_midi_library::analyse::features(&song),
            format: "mid".into(),
            platform: String::new(),
            game: String::new(),
            archive: String::new(),
        };
        return Ok((entry, bytes));
    }
    Ok(resolve(&[name.to_string()])?.remove(0))
}

fn import_one(name: &str, out: &Path, opts: &Options) -> Result<(), String> {
    let (entry, bytes) = resolve_one(name)?;
    let imported = import::import(&bytes, &about(&entry), opts)?;
    std::fs::write(out, &imported.source).map_err(|e| e.to_string())?;
    println!("{} -> {} ({} parts, {} sections, {:.0} beats)", entry.label(), out.display(), imported.parts.len(), imported.sections.len(), imported.beats);
    for p in &imported.parts {
        println!("  {:<10} {:<8} {:>5} notes  {}{}", p.name, p.role.name(), p.notes, p.voice.about(), if p.track_name.is_empty() { String::new() } else { format!("  (\"{}\")", p.track_name) });
    }
    Ok(())
}

/// One cover: the mechanical import, then (with `arrange`) the arranger's
/// Score, rendered at `lufs`; the line for covers.txt.
fn one_cover(entry: &Entry, bytes: &[u8], out: &Path, opts: &Options, lufs: f64, arrange: bool) -> Result<String, String> {
    let started = std::time::Instant::now();
    let slug = format!("{}{}{}", entry.slug(), if opts.gm_only { "_gm" } else { "" }, if arrange { "" } else { "_mechanical" });
    let imported = import::import(bytes, &about(entry), opts)?;
    std::fs::write(out.join(format!("{}_mechanical.splash", entry.slug())), &imported.source).map_err(|e| e.to_string())?;
    let mut similarity = String::new();
    let source = if arrange {
        let writer = makepad_music_iter::gen::ClaudeCode::default();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let arranged = makepad_midi_library::arranger::arrange(&writer, &imported.arrangement, &imported.source, &cancel, &|p| println!("{slug}: {p}"))?;
        similarity = format!(" | {} after {} turn(s)", arranged.similarity.line(), arranged.turns);
        arranged.source
    } else {
        imported.source.clone()
    };
    let r = cover::render(&source, lufs)?;
    std::fs::write(out.join(format!("{slug}.wav")), cover::wav(&r.song)).map_err(|e| e.to_string())?;
    std::fs::write(out.join(format!("{slug}.splash")), &r.source).map_err(|e| e.to_string())?;
    std::fs::write(out.join(format!("{slug}.card.tsv")), r.card.to_tsv()).map_err(|e| e.to_string())?;
    let errors = r.song.mix.errors.len();
    Ok(format!(
        "{slug}: {} | {:.0} s, {} KB of Score (mechanical {} KB) | {:.1} LUFS, {:.1} dBTP | card {}{}{similarity} | {:.0} s",
        entry.label(),
        r.song.mix.left.len() as f64 / r.song.mix.rate as f64,
        r.source.len() / 1000,
        imported.source.len() / 1000,
        r.lufs,
        r.true_peak,
        r.card.compact(),
        if errors > 0 { format!(" | {errors} loop errors") } else { String::new() },
        started.elapsed().as_secs_f64()
    ))
}

fn covers(names: &[String], out: &Path, opts: &Options, lufs: f64, arrange: bool) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let songs = if names.iter().all(|n| n.to_lowercase().ends_with(".mid")) { names.iter().map(|n| resolve_one(n)).collect::<Result<Vec<_>, _>>()? } else { resolve(names)? };
    for (entry, bytes) in songs {
        let line = one_cover(&entry, &bytes, out, opts, lufs, arrange).unwrap_or_else(|e| format!("{}: FAILED {e}", entry.slug()));
        println!("{line}");
        // Each line as it is done: a long run leaves what it finished.
        let path = out.join("covers.txt");
        let mut all = std::fs::read_to_string(&path).unwrap_or_default();
        all.push_str(&line);
        all.push('\n');
        std::fs::write(&path, all).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn references(names: &[String]) -> Result<(), String> {
    for (entry, bytes) in resolve(names)? {
        let path = refs::write(&entry, &bytes, &about(&entry))?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("{} -> {} ({:.1} KB)", entry.label(), path.display(), size as f64 / 1e3);
    }
    println!("listed in {}", refs::list_path().display());
    Ok(())
}

/// The transcription's detail from the flags.
pub fn detail(flag: &dyn Fn(&str) -> Option<String>, all: bool) -> roll::Detail {
    let mut d = roll::Detail::named(flag("--detail").as_deref().unwrap_or("full"));
    if let Some(parts) = flag("--parts").or_else(|| flag("--tracks")) {
        d.parts = Some(parts.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());
    }
    if let Some(bars) = flag("--bars") {
        let (a, b) = bars.split_once('-').unwrap_or((&bars, &bars));
        if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
            d.bars = Some((a, b));
        }
    }
    d.collapse = !all;
    d
}

/// The arrangement of a library song (by id, "Artist - Title" or the best
/// match of search words), a MIDI file or a Score file.
fn arrangement_of(name: &str) -> Result<makepad_midi_library::arrangement::Arrangement, String> {
    let path = Path::new(name);
    if path.is_file() && name.ends_with(".splash") {
        return roll::from_score(&std::fs::read_to_string(path).map_err(|e| e.to_string())?);
    }
    let (entry, bytes) = match resolve_one(name) {
        Ok(found) => found,
        Err(_) => {
            let entries = index::load(&index::index_path())?;
            let (best, _) = index::search(&entries, name, 1).into_iter().next().ok_or_else(|| format!("nothing matches `{name}`"))?;
            resolve(&[best.id.clone()])?.remove(0)
        }
    };
    Ok(import::import(&bytes, &about(&entry), &Options::default())?.arrangement)
}

fn text(name: &str, flag: &dyn Fn(&str) -> Option<String>, all: bool) -> Result<(), String> {
    let a = arrangement_of(name)?;
    print!("{}", roll::to_text(&a, &detail(flag, all)));
    Ok(())
}

fn text_to_score(input: &Path, out: &Path) -> Result<(), String> {
    let a = roll::from_text(&std::fs::read_to_string(input).map_err(|e| e.to_string())?)?;
    let source = makepad_midi_library::arrangement::to_score(&a);
    makepad_score_live::model::load(&source).map_err(|d| format!("the Score does not load: {d:?}"))?;
    std::fs::write(out, &source).map_err(|e| e.to_string())?;
    println!("{} -> {} ({} parts, {} bars)", input.display(), out.display(), a.parts.len(), a.bar_count());
    Ok(())
}
