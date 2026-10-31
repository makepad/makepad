//! The hybrid synth's patch library for `Tandem{patch: "Category/Name"}`
//! instruments: an opt-in download (GPL-3 content, never shipped), described
//! here for the installer and found here by name. Two sets: the current one
//! (revision-31 content, the upstream source archive of a pinned commit,
//! of which only `resources/data/` is unpacked) and the older stable
//! release's content (revision 25, which the loader migrates). The Synth
//! app's Resources install, unpack and verify them; scores only read.

use makepad_audio_synth::tandem::library::{self, Category};
use makepad_model_install::{self as models, Installable};
use std::path::{Path, PathBuf};

/// The licence every set is under.
pub const LICENSE: &str = "GPL-3.0-or-later (patches marked CC0 in their metadata are CC0)";

/// What is shown before install: the licence by name, where to read it in
/// full, what installing means, and the credits per pack.
pub const LICENSE_TEXT: &str = concat!(
    "These patches, wavetables and presets are the reference synth's content: Surge XT (github.com/surge-synthesizer/surge), ",
    "licensed under the GNU General Public License, version 3 or later (GPL-3.0-or-later). The full licence text is at ",
    "https://www.gnu.org/licenses/gpl-3.0.txt and inside the download (LICENSE). You may use, study, change and share ",
    "them; sharing them or works made from them must keep them under the GPL-3 with their source. About 360 third-party ",
    "patches say CC0 in their own metadata (license=\"CC0\"): those are CC0. Synth's own code stays MIT: it reads these ",
    "files, it does not contain them. Nothing is installed until you press Install.\n\n",
    "Credits. Factory library: Claes Johanson (Vember Audio) and the Surge Synth Team (authors in the patches: EvilDragon, ",
    "baconpaul, Fred Cormier, Arty, Macc, Taron, thynctank). Third-party patch folders, each credited to its author: A.Liv, ",
    "Aleksey Zhehanov, Altenberg, Argitoth, Black-Sided Sun, Bluelight, Cybersoda, Damon Armani, Dan Maurer, Databroth, Emu ",
    "(\"The Emu\"), Exquis MPE (Intuitive Instruments), Giana Brotherz (Sola), Inigo Kennedy, Jacky Ligon, John Valentine, ",
    "Kinsey Dulcet, Kuniklo, Kyurumi, Landosonic, LinnStrument MPE (Roger Linn, thoughtForm), Lopyt, Luna, Malfunction, Nick ",
    "Moritz, Noisegeek, Psiome Send Sound, Rare Earth (Leonard Bowman), Rozzer, Slowboat, Stefan Singer, TNMG (The Nerdy ",
    "Music Guy), Vincent Zauhar, Vospi, Xenofish, Zoozither (Polyslax), qb. Wavetable folders: A.Liv, Art of Sound, Damon ",
    "Armani, Emu, Emu VSCO, Layzer, Ocean Swift Synthesis, Philippe Favre, Qonundrai, TNMG, Venus Theory. Factory impulse ",
    "responses: the Surge Synth Team. Not installed: the \"Waldorf\" wavetable folder (no grant on record), skins, and the ",
    "third-party impulse responses (their own licences forbid repackaging)."
);

/// The current set: the upstream source archive of commit dab78c6 (revision 31).
pub const CURRENT: Installable = Installable::dataset(
    "Hybrid patch library (current)",
    LICENSE,
    LICENSE_TEXT,
    "github.com/surge-synthesizer/surge, the source archive of commit dab78c6170732b3883216e569a84bf084ae0ea97 (patch format revision 31); only resources/data/ is unpacked",
    371_531_754,
    "9a81357b0aca41e25b9a95f0fbfd7e9bdfbeea77b4f6533c124d4a87334da3f1",
    "https://github.com/surge-synthesizer/surge/archive/dab78c6170732b3883216e569a84bf084ae0ea97.zip",
    "datasets/tandem/surge-dab78c6.zip",
);

/// The older set: the 1.3.4 release's portable content (revision 25).
pub const STABLE: Installable = Installable::dataset(
    "Hybrid patch library (1.3.4)",
    LICENSE,
    LICENSE_TEXT,
    "github.com/surge-synthesizer/releases-xt, release 1.3.4: surge-xt-portable-content-1.3.4.tar.gz (patch format revision 25, migrated on load)",
    STABLE_BYTES,
    STABLE_SHA256,
    "https://github.com/surge-synthesizer/releases-xt/releases/download/1.3.4/surge-xt-portable-content-1.3.4.tar.gz",
    "datasets/tandem/surge-xt-portable-content-1.3.4.tar.gz",
);
pub const STABLE_BYTES: u64 = 252_763_750;
pub const STABLE_SHA256: &str = "7ed24822eab69f73101d8558222979b1bceb4882a780292bb7087309b7b101bd";

/// Top folders of the data that are unpacked (skins and the third-party
/// impulse responses never are; the "Waldorf" wavetables are left out).
pub const UNPACK_DIRS: [&str; 8] = ["patches_factory", "patches_3rdparty", "wavetables", "wavetables_3rdparty", "fx_presets", "modulator_presets", "tuning_library", "impulses_factory"];
/// Paths under the data folder never unpacked.
pub const EXCLUDED: [&str; 1] = ["wavetables/Waldorf"];

pub fn sets() -> [&'static Installable; 2] {
    [&CURRENT, &STABLE]
}

/// The set's folder: the archive, `data/` (what was unpacked) and its
/// `manifest.tsv` (path, size, SHA-256 per unpacked file).
pub fn dir(set: &Installable) -> PathBuf {
    let archive = models::dest_path(set);
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.trim_end_matches(".zip").trim_end_matches(".tar.gz");
    archive.with_file_name(format!("{stem}.d"))
}

pub fn data_dir(set: &Installable) -> PathBuf {
    dir(set).join("data")
}

pub fn manifest_path(set: &Installable) -> PathBuf {
    dir(set).join("manifest.tsv")
}

pub fn unpacked(set: &Installable) -> bool {
    manifest_path(set).is_file() && data_dir(set).join("patches_factory").is_dir()
}

/// The set scores read: the current one when unpacked, else the older one.
pub fn active() -> Option<&'static Installable> {
    sets().into_iter().find(|s| unpacked(s))
}

/// The active set's categories (factory first, then third party).
pub fn categories() -> Vec<Category> {
    match active() {
        Some(set) => library::scan(&library::data_roots(&data_dir(set)), None),
        None => Vec::new(),
    }
}

/// A patch by `Category/Name` (case-insensitive; a bare name finds the
/// first patch of that name).
pub fn find(name: &str) -> Option<PathBuf> {
    let name = name.trim().trim_start_matches('@');
    let set = active()?;
    find_in(&library::data_roots(&data_dir(set)), name)
}

fn find_in(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    let (cat, stem) = match name.rsplit_once('/') {
        Some((c, s)) => (Some(c), s),
        None => (None, name),
    };
    for root in roots {
        if let Some(cat) = cat {
            let dir = cat.split('/').fold(root.clone(), |p, part| p.join(part));
            if let Some(p) = stem_in(&dir, stem) {
                return Some(p);
            }
        }
    }
    if cat.is_none() {
        for c in library::scan(roots, None) {
            if let Some(p) = c.patches.iter().find(|p| p.name.eq_ignore_ascii_case(stem)) {
                return Some(p.path.clone());
            }
        }
    }
    None
}

fn stem_in(dir: &Path, stem: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fxp")) && p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem))
    })
}
