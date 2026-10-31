//! The General MIDI SoundFont: what a `@soundfont` instrument plays when it
//! names no library SF2 (`Instrument{kind: @soundfont program: @strings}`).
//! It is FluidR3 GM (MIT), installed on demand by Stage's resource installer
//! ([`GM_SOUNDFONT`], an `makepad_model_install::Installable`) into the AI
//! Hub's weights folder, never shipped with the code. A score that plays it
//! and finds it missing says so: offline renders fail, the SYNTH desk offers
//! the install.
//!
//! Where there is no file system (the web), the host fetches the font
//! through the platform's resource path and hands its bytes to
//! [`register`]; [`ensure`] then finds it registered.

use makepad_model_install::{Installable, ModelUse, Root};
use std::path::PathBuf;
use std::sync::Arc;

/// The name the font's bytes are registered under (a library file's name is
/// `id@rev`, so the two never meet).
pub const FONT: &str = "gm:FluidR3_GM";
/// What the installer names it, for messages.
pub const TITLE: &str = "the General MIDI SoundFont (FluidR3 GM)";
/// Its pinned size; the installer checks the SHA-256 before it commits.
pub const BYTES: u64 = 148_398_306;
/// Where the installer puts it under the weights folder.
pub const CACHE_AS: &str = "soundfonts/FluidR3_GM.sf2";

/// FluidR3 GM, Frank Wen's General MIDI SoundFont (release 3, 2008): the
/// 128 GM programs and the GM drum kits Stage's scores play when a
/// `@soundfont` instrument names no library SF2. The file is Debian's
/// `fluid-soundfont` source package, byte for byte the upstream release
/// (the same file the `fluid-soundfont-gm` package installs).
pub const GM_SOUNDFONT: Installable = Installable {
    id: "fluidr3-gm",
    short: "GM SoundFont",
    license: "MIT (FluidR3 GM, Copyright (c) 2000-2002, 2008 Frank Wen)",
    license_text: GM_SOUNDFONT_LICENSE,
    origin: "Frank Wen's Fluid (R3) SoundFont, as Debian's fluid-soundfont 3.1 package ships it (sources.debian.org)",
    use_: ModelUse::Commercial,
    bytes: BYTES,
    sha256: "74594e8f4250680adf590507a306655a299935343583256f3b722c48a1bc1cb0",
    repo: "",
    path: "https://sources.debian.org/data/main/f/fluid-soundfont/3.1-5.3/FluidR3_GM.sf2",
    revision: "",
    cache_as: CACHE_AS,
    root: Root::Weights,
    found: Some(path),
};

/// The FluidR3 licence, verbatim from the package's `COPYING` (its `README`:
/// "I hereby release Fluid under the MIT license, as described in COPYING").
pub const GM_SOUNDFONT_LICENSE: &str = "Copyright (c) 2000-2002, 2008 Frank Wen <getfrank@gmail.com>

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the \"Software\"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.
";

/// Why a score that plays it cannot: said the same everywhere.
pub fn not_installed() -> String {
    format!("{TITLE} is not installed: install it from Resources (Install the GM SoundFont…), then play or render again")
}

/// Where the installer puts the font.
pub fn install_path() -> PathBuf {
    makepad_model_install::dest_path(&GM_SOUNDFONT)
}

/// The font on this machine, if it is there: `MAKEPAD_GM_SOUNDFONT` (taken
/// on trust), else the installer's file at its pinned size (a shorter one is
/// a download that stopped).
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MAKEPAD_GM_SOUNDFONT").filter(|path| !path.is_empty()).map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let path = install_path();
    std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() == BYTES).then_some(path)
}

pub fn installed() -> bool {
    path().is_some()
}

/// Whether the font's bytes are registered for the graphs prepared next.
pub fn registered() -> bool {
    makepad_audio_engine::feed::files::get(FONT).is_some()
}

/// The font's bytes, however the host got them (a file, the platform's
/// resource path on the web), for the graphs prepared next.
pub fn register(bytes: Arc<[u8]>) -> Result<(), String> {
    if !bytes.starts_with(b"RIFF") {
        return Err(format!("{TITLE}: the bytes are not a SoundFont (SF2)"));
    }
    makepad_audio_engine::feed::files::register(FONT, bytes);
    Ok(())
}

/// Read and register the font once, unless the host already registered
/// it. Blocking (140 MB): a worker's job.
pub fn ensure() -> Result<(), String> {
    if registered() {
        return Ok(());
    }
    let path = path().ok_or_else(not_installed)?;
    let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    register(Arc::<[u8]>::from(bytes)).map_err(|_| format!("{} is not a SoundFont (SF2)", path.display()))
}

/// Short names for the programs an arrangement reaches for first.
const ALIASES: [(&str, u8); 30] = [
    ("piano", 0), ("epiano", 4), ("harpsichord", 6), ("celesta", 8), ("glockenspiel", 9), ("vibes", 11), ("marimba", 12), ("bells", 14),
    ("organ", 19), ("accordion", 21), ("guitar", 24), ("bass", 32), ("violin", 40), ("viola", 41), ("cello", 42), ("contrabass", 43),
    ("tremolo", 44), ("pizzicato", 45), ("harp", 46), ("timpani", 47), ("strings", 48), ("choir", 52), ("voice", 53), ("hit", 55),
    ("trumpet", 56), ("trombone", 57), ("tuba", 58), ("horn", 60), ("brass", 61), ("flute", 73),
];

/// A General MIDI program by name: a short alias (`strings`, `brass`,
/// `choir`, `horn` ...) or the GM name in any case with `_` for spaces
/// (`string_ensemble_2`, `french_horn`, `choir_aahs`).
pub fn program(name: &str) -> Option<u8> {
    let key = normalize(name);
    if let Some((_, program)) = ALIASES.iter().find(|(alias, _)| *alias == key) {
        return Some(*program);
    }
    crate::instruments::GM_PROGRAMS.iter().position(|gm| normalize(gm) == key).map(|p| p as u8)
}

/// The names [`program`] knows, aliases first (for completion and errors).
pub fn program_names() -> impl Iterator<Item = String> {
    ALIASES.iter().map(|(alias, _)| alias.to_string()).chain(crate::instruments::GM_PROGRAMS.iter().map(|gm| normalize(gm)))
}

fn normalize(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_resolve_by_alias_and_by_gm_name() {
        assert_eq!(program("strings"), Some(48));
        assert_eq!(program("brass"), Some(61));
        assert_eq!(program("choir"), Some(52));
        assert_eq!(program("French Horn"), Some(60));
        assert_eq!(program("string_ensemble_2"), Some(49));
        assert_eq!(program("bass_lead"), Some(87));
        assert_eq!(program("acoustic_grand"), Some(0));
        assert_eq!(program("kazoo"), None);
        // Every GM name is reachable by its normalized form.
        for (i, gm) in crate::instruments::GM_PROGRAMS.iter().enumerate() {
            let found = program(&normalize(gm)).unwrap();
            assert!(found as usize == i || ALIASES.iter().any(|(a, _)| *a == normalize(gm)), "{gm}");
        }
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn part(program: &str, notes: &str) -> String {
        format!("use mod.score.*\nlet clock = Clock{{bpm: 90}}\nlet gm = Instrument{{kind: @soundfont program: @{program}}}\nlet line = Loop{{play: gm beats: 4 code: fn(i) {{ play({notes}, {{dur: 3}}) sleep(4) }}}}\nScore{{clock gm line}}\n")
    }

    #[test]
    fn gm_parts_render_with_the_font_or_say_why_not() {
        use crate::render::{render, RenderOpts};
        let strings = part("strings", "[@c3 @g3 @e4]");
        let doc = crate::model::ScoreDoc::load(&strings).unwrap_or_else(|d| panic!("{d:?}"));
        assert!(doc.plays_gm_font() && doc.files().is_empty());
        assert!(crate::model::ScoreDoc::load(&part("kazoo", "@c4")).is_err(), "an unknown program name is an error");
        let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
        if !registered() {
            // Nowhere to be found: the render fails and says what to do.
            let saved = std::env::var_os("MAKEPAD_GM_SOUNDFONT");
            std::env::set_var("MAKEPAD_GM_SOUNDFONT", "/nonexistent/FluidR3_GM.sf2");
            let error = render(&strings, &opts, &crate::NoSamples).err().expect("a GM score without the font must not render");
            match saved {
                Some(saved) => std::env::set_var("MAKEPAD_GM_SOUNDFONT", saved),
                None => std::env::remove_var("MAKEPAD_GM_SOUNDFONT"),
            }
            assert!(error[0].message.contains("not installed") && error[0].message.contains("Install the GM SoundFont"), "{error:?}");
        }
        if path().is_none() {
            eprintln!("gm_font: {} is not installed at {}: the render check needs it", TITLE, install_path().display());
            return;
        }
        // Strings, brass and choir, each on its own: every one sounds.
        for (program, notes) in [("strings", "[@c3 @g3 @e4]"), ("brass", "[@f3 @c4 @a4]"), ("choir", "[@a3 @e4 @c5]"), ("french_horn", "@f3")] {
            let r = render(&part(program, notes), &opts, &crate::NoSamples).unwrap_or_else(|d| panic!("{program}: {d:?}"));
            let level = peak(&r.left).max(peak(&r.right));
            eprintln!("gm_font: {program} peak {level:.3}");
            assert!(level > 0.01 && r.left.iter().all(|x| x.is_finite()), "{program} is silent (peak {level})");
        }
    }

    #[test]
    fn the_font_lives_in_the_weights_folder() {
        assert!(install_path().ends_with("weights/soundfonts/FluidR3_GM.sf2") || std::env::var_os("MAKEPAD_ASSET_AI_CACHE").is_some());
        assert!(not_installed().contains("Install the GM SoundFont"));
    }

    #[test]
    fn the_gm_soundfont_lands_in_the_weights_folder() {
        use makepad_model_install::{cache_root, dest_path, weights_root};
        let dest = GM_SOUNDFONT.dest_under(&weights_root());
        assert_eq!(dest, dest_path(&GM_SOUNDFONT));
        assert!(dest.ends_with("soundfonts/FluidR3_GM.sf2"));
        assert!(!dest.starts_with(cache_root()), "a resource outside the checkout");
        assert!(GM_SOUNDFONT.license.starts_with("MIT"));
        assert!(GM_SOUNDFONT.license_text.contains("Permission is hereby granted, free of charge"));
        assert_eq!(GM_SOUNDFONT.use_, ModelUse::Commercial);
        assert_eq!(GM_SOUNDFONT.bytes, BYTES);
    }
}
