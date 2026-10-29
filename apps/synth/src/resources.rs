//! Resources: what Synth downloads on demand instead of shipping. The
//! General MIDI SoundFont its scores play (FluidR3 GM), the MIDI song library
//! (`makepad_midi_library::dataset`: the default set), its research extras
//! (genre annotations and tags, research terms only: never part of the
//! default set, a separate opt-in), and the C64 SID collection. Each is
//! listed with its size, licence (the full text) and origin, fetched by the
//! installer (`makepad_model_install`: a resumable `.part`, the pinned size
//! and SHA-256 checked before a file is committed, a provenance record
//! beside it), verified or removed here. Pressing Install is the
//! acknowledgement of the licence shown with it. Once the song library (or
//! its extras) is installed, its catalogue is built on a worker; the SID
//! collection is unpacked into its pack and catalogue.

use makepad_midi_library::chip::hvsc;
use makepad_midi_library::{dataset, index};
use makepad_model_install::{self as models, InstallHandle, InstallMsg, Installable};
use makepad_score_live::tandem_library;
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let stage = mod.stage
    let Caps = Label{width: Fit text: "" draw_text +: {color: stage.dim text_style: theme.font_bold{font_size: 9}}}
    let Body = Label{width: Fill height: Fit text: "" draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 10}}}
    let Action = StageButton{width: Fit height: 24 padding: Inset{left: 10 right: 10}}
    // One resource: what it is, where it stands, and its actions.
    let Row = View{width: Fill height: Fit flow: Down spacing: 6
        caps := Caps{}
        title := Label{width: Fill height: Fit text: "" draw_text +: {color: stage.text_bright text_style: theme.font_bold{font_size: 11}}}
        about := Body{}
        state := Label{width: Fill height: Fit text: "" draw_text +: {color: stage.info text_style: theme.font_regular{font_size: 10}}}
        View{width: Fill height: Fit flow: Right spacing: 8
            Tip{text: "Download it (you accept its licence, shown with Licence)" install := Action{text: "Install"}}
            Tip{text: "Stop the download; Install resumes it" stop := Action{text: "Stop"}}
            Tip{text: "Check the files on disk against their pinned SHA-256" verify := Action{text: "Verify"}}
            Tip{text: "Delete the installed files (what uses them then asks for them again)" remove := Action{text: "Remove"}}
            View{width: Fill height: 1}
            Tip{text: "Show the licence text and where the files come from" licence := Action{text: "Licence"}}
        }
        licence_box := ScrollYView{visible: false width: Fill height: 220 show_bg: true draw_bg +: {color: stage.well}
            licence_text := Label{width: Fill height: Fit padding: 10 text: ""
                draw_text +: {color: stage.label text_style: theme.font_code{font_size: 8.5}}}}
    }
    mod.widgets.SynthResourcesPanel = Modal{
        content +: {width: 580 height: Fit flow: Down spacing: 10 padding: 18 show_bg: true
            draw_bg +: {
                color: stage.panel
                pixel: fn(){
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0., 0., self.rect_size.x, self.rect_size.y, 8.)
                    sdf.fill(vec4(self.color.xyz, 1.))
                    return sdf.result
                }
            }
            View{width: Fill height: Fit flow: Right spacing: 8
                Label{width: Fill text: "Resources" draw_text +: {color: stage.text_bright text_style: theme.font_bold{font_size: 14}}}
                resources_close := StageButton{width: 56 height: 24 text: "Close"}
            }
            Body{text: "Files Synth fetches when you ask instead of shipping them: each is checked against its pinned SHA-256 before it is kept, a stopped download resumes where it left off, and Remove deletes it again."}
            ScrollYView{width: Fill height: 520 flow: Down spacing: 14
                resources_gm := Row{}
                resources_midi := Row{}
                resources_research := Row{}
                resources_sid := Row{}
                resources_patches := Row{}
                resources_patches_old := Row{}
            }
        }
    }
}

/// The rows: the SoundFont, the song library, its research extras, the SID
/// collection.
const ROWS: [LiveId; 6] = [
    live_id!(resources_gm),
    live_id!(resources_midi),
    live_id!(resources_research),
    live_id!(resources_sid),
    live_id!(resources_patches),
    live_id!(resources_patches_old),
];
const GM_ROW: usize = 0;
const MIDI: usize = 1;
const RESEARCH: usize = 2;
const SID: usize = 3;
/// The hybrid patch library: the current set and the older stable one.
const PATCHES: usize = 4;
const PATCHES_OLD: usize = 5;

fn patch_set(row: usize) -> &'static Installable {
    if row == PATCHES { &tandem_library::CURRENT } else { &tandem_library::STABLE }
}

const GM: &Installable = &makepad_score_live::gm_font::GM_SOUNDFONT;

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// A row's files.
fn files(row: usize) -> Vec<&'static Installable> {
    match row {
        GM_ROW => vec![GM],
        MIDI => dataset::FILES.to_vec(),
        RESEARCH => dataset::RESEARCH.to_vec(),
        PATCHES | PATCHES_OLD => vec![patch_set(row)],
        _ => vec![&hvsc::ARCHIVE],
    }
}

fn installed(row: usize) -> bool {
    files(row).iter().all(|model| models::installed(model))
}

/// A file's name, for messages.
fn file_name(model: &Installable) -> &str {
    model.cache_as.rsplit('/').next().unwrap_or(model.cache_as)
}

/// What a worker reports: (done, total) as it goes, then its end.
type Progress<T> = Receiver<(u64, u64, Option<Result<T, String>>)>;

enum Job {
    Install(InstallHandle),
    Verify(Progress<()>),
    /// The song library's catalogue being built: the songs indexed.
    Index(Progress<usize>),
    /// The SID collection unpacked: the tunes packed.
    Unpack(Progress<usize>),
    /// A patch library unpacked: its patches.
    Patches(Progress<usize>),
    /// A patch library's unpacked files checked against its manifest.
    PatchesVerify(Progress<usize>),
}

/// Run `work` on a worker that reports its progress.
fn worker<T: Send + 'static>(name: &str, work: impl FnOnce(&(dyn Fn(usize, usize) + Sync)) -> Result<T, String> + Send + 'static) -> Progress<T> {
    let (out, rx) = channel();
    std::thread::Builder::new().name(name.into()).spawn(move || {
        let progress = std::sync::Mutex::new(out.clone());
        let result = work(&|done, total| {
            if let Ok(out) = progress.lock() {
                let _ = out.send((done as u64, total as u64, None));
            }
        });
        let _ = out.send((0, 0, Some(result)));
    }).ok();
    rx
}

/// Build the song library's catalogue on a worker.
fn start_index() -> Job {
    Job::Index(worker("song-index", |progress| index::build(4, progress).and_then(|entries| index::write(&entries, &index::index_path()).map(|_| entries.len()))))
}

/// Unpack the SID collection on a worker.
fn start_unpack() -> Job {
    Job::Unpack(worker("sid-unpack", |progress| hvsc::unpack(4, progress)))
}

/// Unpack a patch library on a worker.
fn start_patches(row: usize) -> Job {
    Job::Patches(worker("patch-unpack", move |progress| crate::patch_library::unpack(patch_set(row), progress)))
}

/// Whether the song library's catalogue is built.
fn indexed() -> bool {
    index::index_path().is_file()
}

#[derive(Default)]
struct RowState {
    /// The last probe (a stat per file), refreshed on open and when a job ends.
    installed: Option<bool>,
    note: String,
    licence: bool,
}

#[derive(Default)]
pub struct Resources {
    /// The one job running and the row it is for.
    job: Option<(usize, Job)>,
    progress: Option<(u64, u64)>,
    rows: [RowState; ROWS.len()],
    /// The GM SoundFont was installed or removed since the last look.
    changed: bool,
    /// The song library's catalogue was built since the last look.
    catalogued: bool,
    timer: Timer,
}

impl Resources {
    /// The GM SoundFont was installed or removed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// The song library's catalogue was (re)built since the last call.
    pub fn take_catalogued(&mut self) -> bool {
        std::mem::take(&mut self.catalogued)
    }

    pub fn open(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        for row in 0..ROWS.len() {
            self.rows[row].installed = Some(installed(row));
        }
        // A library installed but not catalogued (installed elsewhere, or a
        // catalogue run that stopped): build it now; the same for the SID
        // collection's pack.
        if self.job.is_none() && self.rows[MIDI].installed == Some(true) && !indexed() {
            self.job = Some((MIDI, start_index()));
            self.run(cx);
        } else if self.job.is_none() && self.rows[SID].installed == Some(true) && !hvsc::unpacked() {
            self.job = Some((SID, start_unpack()));
            self.run(cx);
        } else if let Some(row) = [PATCHES, PATCHES_OLD].into_iter().find(|r| self.job.is_none() && self.rows[*r].installed == Some(true) && !tandem_library::unpacked(patch_set(*r))) {
            self.job = Some((row, start_patches(row)));
            self.run(cx);
        }
        self.sync(cx, ui);
        ui.modal(cx, ids!(resources_panel)).open(cx);
    }

    pub fn stop(&mut self) {
        if let Some((_, Job::Install(install))) = &self.job {
            install.cancel();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: &Event) {
        if self.timer.is_event(event).is_some() {
            self.pump(cx, ui);
        }
        if let Event::Shutdown = event {
            self.stop();
        }
    }

    pub fn handle_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        if ui.button(cx, ids!(resources_close)).clicked(actions) {
            ui.modal(cx, ids!(resources_panel)).close(cx);
        }
        for (row, id) in ROWS.iter().copied().enumerate() {
            if ui.button(cx, ids!((id).licence)).clicked(actions) {
                self.rows[row].licence = !self.rows[row].licence;
                self.sync(cx, ui);
            }
            let idle = self.job.is_none();
            let have = *self.rows[row].installed.get_or_insert_with(|| installed(row));
            if ui.button(cx, ids!((id).install)).clicked(actions) && idle && !have {
                self.progress = None;
                self.rows[row].note = String::new();
                let missing = files(row).into_iter().filter(|model| !models::installed(model)).collect();
                self.job = Some((row, Job::Install(models::start_install(missing, Some(models::unix_now())))));
                self.run(cx);
                self.sync(cx, ui);
            }
            if ui.button(cx, ids!((id).stop)).clicked(actions) && matches!(self.job, Some((r, _)) if r == row) {
                self.stop();
            }
            if ui.button(cx, ids!((id).verify)).clicked(actions) && idle && have && matches!(row, PATCHES | PATCHES_OLD) {
                let rx = worker("patch-verify", move |progress| {
                    models::verify(patch_set(row), &mut |_, _| {}).map_err(|error| format!("{}: {error}", file_name(patch_set(row))))?;
                    crate::patch_library::verify(patch_set(row), progress)
                });
                self.job = Some((row, Job::PatchesVerify(rx)));
                self.rows[row].note = "verifying…".into();
                self.run(cx);
                self.sync(cx, ui);
            } else if ui.button(cx, ids!((id).verify)).clicked(actions) && idle && have {
                let rx = worker("resource-verify", move |_| {
                    for model in files(row) {
                        models::verify(model, &mut |_, _| {}).map_err(|error| format!("{}: {error}", file_name(model)))?;
                    }
                    Ok(())
                });
                self.job = Some((row, Job::Verify(rx)));
                self.rows[row].note = "verifying…".into();
                self.run(cx);
                self.sync(cx, ui);
            }
            if ui.button(cx, ids!((id).remove)).clicked(actions) && idle {
                let mut failed: Vec<String> = files(row).into_iter().filter_map(|model| models::uninstall(model).err()).collect();
                if matches!(row, PATCHES | PATCHES_OLD) {
                    if let Err(error) = crate::patch_library::remove_unpacked(patch_set(row)) {
                        failed.push(error);
                    }
                }
                let made = match row {
                    MIDI => vec![index::index_path()],
                    SID => vec![hvsc::pack_path(), hvsc::offsets_path(), hvsc::catalogue_path()],
                    _ => Vec::new(),
                };
                for path in made.into_iter().filter(|p| p.is_file()) {
                    if let Err(error) = std::fs::remove_file(&path) {
                        failed.push(format!("{}: {error}", path.display()));
                    }
                }
                self.rows[row].note = if failed.is_empty() { "removed".into() } else { failed.join("; ") };
                if matches!(row, MIDI | SID) {
                    self.catalogued = true;
                }
                self.probe(row);
                self.sync(cx, ui);
            }
        }
    }

    fn run(&mut self, cx: &mut Cx) {
        if self.timer.is_empty() {
            self.timer = cx.start_interval(0.2);
        }
    }

    fn probe(&mut self, row: usize) {
        let now = installed(row);
        if row == GM_ROW && self.rows[GM_ROW].installed != Some(now) {
            self.changed = true;
        }
        self.rows[row].installed = Some(now);
    }

    /// A worker's progress in; its end as the row's note.
    fn drain<T>(rx: &Progress<T>, progress: &mut Option<(u64, u64)>, done: impl FnOnce(Result<T, String>) -> String) -> Option<String> {
        loop {
            match rx.try_recv() {
                Ok((at, total, None)) => *progress = Some((at, total)),
                Ok((_, _, Some(result))) => return Some(done(result)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some("stopped".into()),
            }
        }
    }

    fn pump(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let mut finished = None;
        if let Some((row, job)) = &self.job {
            let row = *row;
            let note = match job {
                Job::Install(install) => {
                    let mut end = None;
                    loop {
                        match install.rx.try_recv() {
                            Ok(InstallMsg::Progress { done, total, .. }) => self.progress = Some((done, total)),
                            Ok(InstallMsg::Done { short }) => self.rows[row].note = format!("{short}: installed and verified"),
                            Ok(InstallMsg::Failed { short, error }) => self.rows[row].note = format!("{short}: {error}"),
                            Ok(InstallMsg::Cancelled) => self.rows[row].note = "stopped: Install resumes where it left off".into(),
                            Ok(InstallMsg::Finished) | Err(TryRecvError::Disconnected) => {
                                end = Some(self.rows[row].note.clone());
                                break;
                            }
                            Err(TryRecvError::Empty) => break,
                        }
                    }
                    end
                }
                Job::Index(rx) => Self::drain(rx, &mut self.progress, |result| match result {
                    Ok(songs) => format!("{songs} songs catalogued: search them in the Song library"),
                    Err(error) => format!("not catalogued: {error}"),
                }),
                Job::Unpack(rx) => Self::drain(rx, &mut self.progress, |result| match result {
                    Ok(tunes) => format!("{tunes} tunes unpacked: search them in the Song library (sid, c64)"),
                    Err(error) => format!("not unpacked: {error}"),
                }),
                Job::Patches(rx) => Self::drain(rx, &mut self.progress, |result| match result {
                    Ok(patches) => format!("{patches} patches unpacked: play them with Tandem{{patch: …}} (tandem_patches finds them)"),
                    Err(error) => format!("not unpacked: {error}"),
                }),
                Job::PatchesVerify(rx) => Self::drain(rx, &mut self.progress, |result| match result {
                    Ok(files) => format!("verified: the archive and all {files} unpacked files match"),
                    Err(error) => format!("verify failed: {error}"),
                }),
                Job::Verify(rx) => Self::drain(rx, &mut self.progress, |result| match result {
                    Ok(()) => "verified: the SHA-256 matches".into(),
                    Err(error) => format!("verify failed: {error}"),
                }),
            };
            if let Some(note) = note {
                finished = Some((row, note));
            }
        }
        if let Some((row, note)) = finished {
            let was_install = matches!(self.job, Some((_, Job::Install(_))));
            let built = matches!(self.job, Some((_, Job::Index(_) | Job::Unpack(_) | Job::Patches(_))));
            self.rows[row].note = note;
            self.job = None;
            self.progress = None;
            self.probe(row);
            if built {
                self.catalogued = true;
            }
            // The song library (or its extras) installed: catalogue it; the
            // SID collection: unpack it.
            if was_install && (row == MIDI || (row == RESEARCH && self.rows[MIDI].installed == Some(true))) && self.rows[row].installed == Some(true) {
                self.job = Some((MIDI, start_index()));
            } else if was_install && row == SID && self.rows[SID].installed == Some(true) {
                self.job = Some((SID, start_unpack()));
            } else if was_install && matches!(row, PATCHES | PATCHES_OLD) && self.rows[row].installed == Some(true) {
                self.job = Some((row, start_patches(row)));
            }
        }
        if self.job.is_none() {
            cx.stop_timer(self.timer);
            self.timer = Timer::empty();
        }
        self.sync(cx, ui);
    }

    fn sync(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        for (row, id) in ROWS.iter().copied().enumerate() {
            let list = files(row);
            let bytes: u64 = list.iter().map(|model| model.bytes).sum();
            let sources = || -> Vec<String> {
                list.iter().map(|model| format!("{} ({})\n  {}\n  Licence: {}\n  Source: {}\n  SHA-256 {} · {} bytes\n\n{}", model.short, file_name(model), model.origin, model.license, models::source_url(model), model.sha256, model.bytes, model.license_text)).collect()
            };
            let (caps, title, about, licence) = match row {
                GM_ROW => (
                    "GENERAL MIDI SOUNDFONT".to_string(),
                    format!("FluidR3 GM · {} · MIT", megabytes(bytes)),
                    format!(
                        "The 128 General MIDI instruments and the GM drum kits, sampled: what a score's Instrument{{kind: @soundfont}} plays when it names no SF2 of its own (program: @strings, @brass, @choir…), and what imported songs play. Licence: {}. Kept at {}.",
                        GM.license,
                        models::dest_path(GM).display(),
                    ),
                    format!("{}\n\nOrigin: {}\nSource: {}\nSHA-256: {}\nSize: {} bytes", GM.license_text, GM.origin, models::source_url(GM), GM.sha256, GM.bytes),
                ),
                MIDI => (
                    "SONG LIBRARY".to_string(),
                    format!("MIDI song library · {} files · {} · CC BY 4.0 and research terms", list.len(), megabytes(bytes)),
                    format!("About 17,000 songs as MIDI with their years: search them in the Song library and open one as a score. {} Kept at {}.", dataset::NOTICE, dataset::dir().display()),
                    format!("{}\n\n{}", dataset::NOTICE, sources().join("\n\n")),
                ),
                RESEARCH => (
                    "SONG LIBRARY: RESEARCH EXTRAS".to_string(),
                    format!("Genres and tags · {} files · {} · research, non-commercial only", list.len(), megabytes(bytes)),
                    "Genre annotations and listeners' tags for the song library's songs, so searches by genre and mood find more. Their terms allow research and non-commercial use only: they are not part of the song library's download, and the library works without them.".to_string(),
                    sources().join("\n\n"),
                ),
                PATCHES | PATCHES_OLD => {
                    let set = patch_set(row);
                    let (caps, which) = if row == PATCHES {
                        ("HYBRID PATCH LIBRARY", "Surge XT's current patches, wavetables and presets (patch format revision 31, upstream commit dab78c6)")
                    } else {
                        ("HYBRID PATCH LIBRARY: OLDER RELEASE", "Surge XT 1.3.4's content (revision 25, migrated as it loads); install it only if you want that release's set")
                    };
                    (
                        caps.to_string(),
                        format!("Surge XT patch library · {} · GPL-3.0-or-later", megabytes(bytes)),
                        format!(
                            "{which}: about 3,500 factory and third-party patches that Synth's Tandem instrument plays (`Tandem{{patch: \"Leads/Name\"}}`), unpacked to {} with a hash of every file. GPL-3: Synth does not ship them; you download them here.",
                            tandem_library::data_dir(set).display()
                        ),
                        sources().join("\n\n"),
                    )
                }
                _ => (
                    "C64 SID COLLECTION".to_string(),
                    format!("Commodore 64 tunes · {} · {}", megabytes(bytes), hvsc::LICENSE),
                    format!("Every known C64 tune, with its composer, title and release: search them in the Song library (sid, c64, a composer) and open one as a score. Kept at {}.", hvsc::dir().display()),
                    sources().join("\n\n"),
                ),
            };
            let state = &self.rows[row];
            let have = state.installed.unwrap_or(false);
            let mine = matches!(self.job, Some((r, _)) if r == row);
            let busy = self.job.is_some();
            let text = match (self.progress.filter(|_| mine), &self.job) {
                (Some((done, total)), Some((_, Job::Install(_)))) => format!("downloading {}% ({} of {})", done * 100 / total.max(1), megabytes(done), megabytes(total)),
                (None, Some((_, Job::Install(_)))) if mine => "starting the download…".into(),
                (Some((done, total)), Some((_, Job::Verify(_)))) => format!("verifying {}%", done * 100 / total.max(1)),
                (Some((done, total)), Some((_, Job::Index(_)))) if mine => format!("installed · cataloguing {done}/{total} songs"),
                (None, Some((_, Job::Index(_)))) if mine => "installed · cataloguing the songs…".into(),
                (Some((done, total)), Some((_, Job::Unpack(_)))) if mine => format!("installed · unpacking {done}/{total} tunes"),
                (None, Some((_, Job::Unpack(_)))) if mine => "installed · unpacking the tunes…".into(),
                _ if have && row == MIDI && state.note.is_empty() && !indexed() => "installed · not catalogued yet".into(),
                _ if have && row == SID && state.note.is_empty() && !hvsc::unpacked() => "installed · not unpacked yet".into(),
                (Some((done, total)), Some((_, Job::Patches(_)))) if mine => format!("installed · unpacking {done}/{total} files"),
                (None, Some((_, Job::Patches(_)))) if mine => "installed · unpacking…".into(),
                (Some((done, total)), Some((_, Job::PatchesVerify(_)))) if mine => format!("verifying {done}/{total} files"),
                _ if have && matches!(row, PATCHES | PATCHES_OLD) && state.note.is_empty() && !tandem_library::unpacked(patch_set(row)) => "installed · not unpacked yet".into(),
                _ if have && state.note.is_empty() => "installed: every file at its pinned size".into(),
                _ if have => format!("installed · {}", state.note),
                _ if state.note.is_empty() => "not installed".into(),
                _ => format!("not installed · {}", state.note),
            };
            ui.label(cx, ids!((id).caps)).set_text(cx, &caps);
            ui.label(cx, ids!((id).title)).set_text(cx, &title);
            ui.label(cx, ids!((id).about)).set_text(cx, &about);
            ui.label(cx, ids!((id).state)).set_text(cx, &text);
            ui.button(cx, ids!((id).install)).set_visible(cx, !have && !busy);
            ui.button(cx, ids!((id).stop)).set_visible(cx, mine && matches!(self.job, Some((_, Job::Install(_)))));
            ui.button(cx, ids!((id).verify)).set_visible(cx, have && !busy);
            ui.button(cx, ids!((id).remove)).set_visible(cx, have && !busy);
            ui.view(cx, ids!((id).licence_box)).set_visible(cx, state.licence);
            if state.licence {
                ui.label(cx, ids!((id).licence_text)).set_text(cx, &licence);
            }
        }
        ui.redraw(cx);
    }
}
