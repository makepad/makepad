//! One application state shared through `Scope::with_data` by every score UI
//! widget. UI actions funnel through [`apply_score_action`]; canvas hover and
//! scrub are the intentionally direct hot paths.

use crate::{
    action::*,
    document::{DocumentError, DragTarget, NoteDrag, PartUiState, ScoreDocument},
    library::MusicLibrary,
    playback::PlaybackBridge,
    prefs::ScorePrefs,
    sound::{self, SoundParam, SoundSettings},
};
use crate::sound::{InstrumentId, ScoreEngine};
use makepad_score::{model::AnnotationKind, symbol::Articulation};
use makepad_score_play::PlaybackState;
use makepad_score_render::{PageId, PlaybackPosition, SemanticId};
use makepad_widgets::*;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct SelectionState {
    pub ordered: Vec<SemanticId>,
    pub active: Option<SemanticId>,
}

impl SelectionState {
    fn replace(&mut self, semantic: SemanticId) {
        self.ordered.clear();
        self.ordered.push(semantic);
        self.active = Some(semantic);
    }

    fn clear(&mut self) {
        self.ordered.clear();
        self.active = None;
    }
}

/// How long the transient pianist controls stay up after the last pointer
/// movement. Long enough to actually reach and press a button — a reader that
/// hides its controls the instant the pointer stops is unusable.
pub const CONTROLS_DWELL_S: f64 = 2.6;

/// The view gliding to a page somebody asked for by name — a menu, a key, the
/// transport, the playback cursor. Panning by hand does not use it: dragging
/// *is* the movement, and a glide would fight it.
///
/// The target is a page rather than a point, so the glide stays correct if the
/// window resizes or the zoom changes while it runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageGlide {
    pub page: usize,
    /// Where the view was when the glide started, in window points.
    pub from: DVec2,
    pub progress: f64,
    pub active: bool,
}

/// How long the view takes to glide to a page it was sent to.
pub const PAGE_GLIDE_S: f64 = 0.28;

/// The zoom range the whole application agrees on. The bottom is "the entire
/// document as a row of thumbnails", the top is a magnifier over one system.
pub const ZOOM_MIN: f64 = 0.05;
pub const ZOOM_MAX: f64 = 6.0;

#[derive(Clone, Debug)]
pub struct ScoreUiState {
    pub mode: ProductMode,
    pub chrome_visible: bool,
    pub controls_visible: bool,
    /// Wall clock (`Cx::time_now`) after which the transient controls may fade.
    pub controls_hold_until: f64,
    /// The pointer is on the control strip itself. Never fade while it is.
    pub controls_pinned: bool,
    /// What a drag on the page means. Navigate by default and after Escape,
    /// so the resting state of the application cannot change the music.
    pub tool: ScoreTool,
    pub page_layout: PageLayout,
    /// The page the reader is actually looking at. Derived from the view
    /// wherever the view moved on its own (a grab-pan, a scrollbar, a zoom),
    /// and set directly by anything that *sends* the reader to a page.
    pub current_page: usize,
    pub zoom: f64,
    /// Where the document sits in the viewport, in window points: the offset
    /// of the document's origin from the canvas's top-left corner. One offset
    /// for every layout, shared by grab-pan, both scrollbars, the zoom and the
    /// page glide, so none of them can disagree about where the reader is.
    pub pan: DVec2,
    /// A glide towards a page the reader was sent to.
    pub glide: PageGlide,
    /// Ask the canvas to put the current page back under the reader without
    /// animating: a fresh document, a new layout, Fit page.
    pub recentre: bool,
    /// Ask the canvas for the zoom that holds the whole document. The zoom is
    /// geometry the canvas owns, so the request travels rather than the value.
    pub fit_all: bool,
    /// A zoom gesture, or the ease that finishes one, is in flight. The
    /// playback follow stands down while it is: see `sync_follow_page`.
    pub zooming: bool,
    pub selection: SelectionState,
    pub hover: Option<SemanticId>,
    pub caret: Option<SemanticId>,
    pub entry_duration: u8,
    pub shadow_pitch: Option<u8>,
    pub annotation_tool: AnnotationTool,
    pub pending_annotation_target: Option<SemanticId>,
    pub inspector_tab: InspectorTab,
    /// The sound control last touched, so the panel can explain that one
    /// rather than showing sixteen paragraphs at once.
    pub sound_focus: Option<SoundParam>,
    pub dialog: DialogKind,
    pub context_menu_at: Option<DVec2>,
    pub context_semantic: Option<SemanticId>,
    pub status: String,
    pub text_input_focused: bool,
    /// What the open dialog is about to apply, and what went wrong last time.
    pub draft: DialogDraft,
    pub dialog_error: Option<String>,
}

/// The pending, not-yet-applied values of the setup dialogs. Keeping them out
/// of the live UI state is what lets Cancel mean cancel.
#[derive(Clone, Copy, Debug, Default)]
pub struct DialogDraft {
    pub layout: PageLayout,
    pub zoom: f64,
    pub tempo: f64,
}

impl Default for ScoreUiState {
    fn default() -> Self {
        Self {
            mode: ProductMode::Pianist,
            chrome_visible: false,
            controls_visible: false,
            controls_hold_until: 0.0,
            controls_pinned: false,
            tool: ScoreTool::Navigate,
            page_layout: PageLayout::Single,
            current_page: 0,
            zoom: 1.0,
            pan: DVec2::default(),
            glide: PageGlide::default(),
            recentre: true,
            fit_all: false,
            zooming: false,
            selection: SelectionState::default(),
            hover: None,
            caret: None,
            entry_duration: 5,
            shadow_pitch: None,
            annotation_tool: AnnotationTool::None,
            pending_annotation_target: None,
            inspector_tab: InspectorTab::Properties,
            sound_focus: None,
            dialog: DialogKind::None,
            context_menu_at: None,
            context_semantic: None,
            status: "Pianist mode · move the pointer for controls · ⌘E to edit".into(),
            text_input_focused: false,
            draft: DialogDraft {
                layout: PageLayout::Single,
                zoom: 1.0,
                tempo: 108.0,
            },
            dialog_error: None,
        }
    }
}

impl ScoreUiState {
    /// The face the application opens with. The editor by default — it is the
    /// whole instrument, and pianist mode is one keystroke away from it — but
    /// a reader who chose the pianist face keeps it.
    pub fn from_prefs(prefs: &ScorePrefs) -> Self {
        let mut ui = Self::default();
        if prefs.start_in_editor {
            ui.mode = ProductMode::Editor;
            ui.chrome_visible = true;
            ui.status = "Editor mode · notation tools revealed".into();
        }
        ui
    }

    /// Send the reader to a page: the view glides there rather than cutting,
    /// and `current_page` is the request until the glide lands. Every command
    /// that names a page goes through here, so the header, the scrollbars and
    /// the paper can never be looking at different pages.
    pub fn go_to_page(&mut self, page: usize) {
        if self.current_page == page && self.glide.active {
            return;
        }
        self.current_page = page;
        self.glide = PageGlide {
            page,
            from: self.pan,
            progress: 0.0,
            active: true,
        };
    }

    /// Put the current page back under the reader with no animation at all:
    /// a document that just opened, a layout that just changed, Fit page.
    pub fn recentre_now(&mut self) {
        self.glide.active = false;
        self.recentre = true;
    }

    /// Any pointer movement brings the controls up and restarts the dwell.
    /// Returns true when the visible state actually changed.
    pub fn reveal_controls(&mut self, now: f64) -> bool {
        let changed = !self.controls_visible;
        self.controls_visible = true;
        self.controls_hold_until = now + CONTROLS_DWELL_S;
        changed
    }

    /// Called from the dwell timer. Returns true when the controls just hid.
    pub fn tick_controls(&mut self, now: f64) -> bool {
        if !self.controls_visible {
            return false;
        }
        if self.controls_pinned {
            // Sitting on the strip holds it open indefinitely.
            self.controls_hold_until = now + CONTROLS_DWELL_S;
            return false;
        }
        if now >= self.controls_hold_until {
            self.controls_visible = false;
            return true;
        }
        false
    }

    /// True while a hide is still owed, so the dwell timer keeps running.
    pub fn controls_hide_pending(&self) -> bool {
        self.controls_visible && !self.controls_pinned
    }
}

#[derive(Clone, Debug)]
pub struct PracticeState {
    pub tempo: f64,
    pub metronome: bool,
    pub count_in: bool,
    pub follow_cursor: bool,
    pub loop_enabled: bool,
    pub loop_start_quarter: f64,
    pub loop_end_quarter: f64,
    pub playing: bool,
    /// When Play was last asked for. The engine takes an audio block or two to
    /// answer, so the published clock is only trusted after this much grace.
    pub play_requested_at: f64,
}

/// How long the UI waits for the engine to confirm a transport request before
/// it starts believing the published clock instead of its own flag.
pub const TRANSPORT_GRACE_S: f64 = 0.5;

impl Default for PracticeState {
    fn default() -> Self {
        Self {
            tempo: 108.0,
            metronome: false,
            count_in: false,
            follow_cursor: true,
            loop_enabled: false,
            loop_start_quarter: 0.0,
            loop_end_quarter: 4.0,
            playing: false,
            play_requested_at: f64::NEG_INFINITY,
        }
    }
}

impl PracticeState {
    /// Reconcile the UI's transport flag with what the engine actually
    /// publishes. A piece that reaches its end stops in the engine; without
    /// this the button still read "Pause", so the next press sent Pause to an
    /// already-stopped engine and it took two presses to hear anything.
    ///
    /// Returns true when the flag moved, so the caller can redraw.
    pub fn sync_transport(&mut self, engine_playing: bool, now: f64) -> bool {
        if now - self.play_requested_at < TRANSPORT_GRACE_S {
            return false;
        }
        if self.playing != engine_playing {
            self.playing = engine_playing;
            return true;
        }
        false
    }
}

pub struct ScoreAppState {
    pub document: ScoreDocument,
    pub ui: ScoreUiState,
    pub practice: PracticeState,
    /// Everything that shapes the piano, as the sound panel edits it.
    pub sound: SoundSettings,
    pub playback: PlaybackBridge,
    pub parts: Vec<PartUiState>,
    pub prefs: ScorePrefs,
    /// The browsable folder of scores; scanned the first time it is opened.
    pub library: MusicLibrary,
    /// Which page of the library list the browser is showing.
    pub library_page: usize,
    /// Who played the open piece, when it is somebody's performance. Shown in
    /// About and on the status line; `None` for an engraving.
    pub performance_credit: Option<&'static str>,
    midi_input: MidiInput,
    midi_output: MidiOutput,
    midi_ready: bool,
    midi_ports: usize,
    midi_hover: Vec<u8>,
    /// Hidden verification instances must stay silent, MIDI included.
    midi_muted: bool,
    /// A hover is sounding, so it can be released when the piece starts.
    hover_sounding: bool,
    /// The device rate the current instrument was built for. `None` until the
    /// audio callback has reported one.
    instrument_rate: Option<f32>,
}

impl Default for ScoreAppState {
    fn default() -> Self {
        let document = ScoreDocument::default();
        let prefs = ScorePrefs::load();
        let mut practice = PracticeState::default();
        practice.metronome = prefs.metronome;
        practice.count_in = prefs.count_in;
        practice.follow_cursor = prefs.follow_cursor;
        let playback = PlaybackBridge::new(document.score(), practice.tempo, practice.count_in);
        let parts = document.parts();
        // The stored instrument, or the app default when nothing is stored.
        // A name that is no longer in the shipped list falls back rather than
        // resolving to whatever now sits at that index.
        let mut sound = SoundSettings::default();
        sound.engine = prefs.engine();
        if let Some(preset) = sound::preset_index_by_name(sound.engine, &prefs.instrument) {
            sound.apply_preset(preset);
        }
        // The two exposed controls outlive the session too; they are applied
        // after the instrument, which supplies the room they sit on.
        if let Some(room) = crate::prefs::reverb_preset_by_name(&prefs.room) {
            sound.room.preset = room;
        }
        if let Some(amount) = prefs.reverb {
            SoundParam::Reverb.set(&mut sound, amount);
        }
        SoundParam::Brightness.set(&mut sound, prefs.brightness);
        let mut ui = ScoreUiState::from_prefs(&prefs);
        ui.draft.tempo = practice.tempo;
        let library = MusicLibrary::new(prefs.library_dir.as_deref());
        Self {
            document,
            ui,
            practice,
            sound,
            playback,
            parts,
            prefs,
            library,
            library_page: 0,
            performance_credit: None,
            midi_input: MidiInput::default(),
            midi_output: MidiOutput::default(),
            midi_ready: false,
            midi_ports: 0,
            midi_hover: Vec::new(),
            midi_muted: std::env::var_os("MAKEPAD_SCORE_MUTE").is_some(),
            hover_sounding: false,
            instrument_rate: None,
        }
    }
}

impl ScoreAppState {
    pub fn install_io(&mut self, cx: &mut Cx) {
        self.playback.install_audio_output(cx);
        self.playback.set_sound(self.sound);
        if !self.midi_ready {
            self.midi_input = cx.midi_input();
            self.midi_output = cx.midi_output();
            self.midi_ready = true;
        }
    }

    /// Open the piece the application ships inside its own executable.
    ///
    /// A resource that cannot be read costs the shipped piece and nothing
    /// else: the built-in demo score stays on the desk and the status line
    /// says why. Returns whether the shipped piece is what is now open.
    pub fn open_bundled_score(
        &mut self,
        cx: &mut Cx,
        bytes: &[u8],
        extension: &str,
        title: &str,
        credit: &str,
    ) -> bool {
        match ScoreDocument::open_bundled(bytes, extension, title) {
            Ok(document) => {
                self.document = document;
                self.parts = self.document.parts();
                self.ui.current_page = 0;
                self.ui.selection.clear();
                self.ui.recentre_now();
                self.adopt_score_tempo();
                self.rebuild_playback(cx);
                self.ui.status = credit.to_string();
                true
            }
            Err(error) => {
                self.ui.status =
                    format!("{title} could not be read ({error}) · built-in demo score");
                false
            }
        }
    }

    pub fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }

    pub fn handle_midi_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        let inputs = ports.all_inputs();
        let outputs = ports.all_outputs();
        self.midi_ports = inputs.len() + outputs.len();
        cx.use_midi_inputs(&inputs);
        cx.use_midi_outputs(&outputs);
        self.ui.status = if self.midi_ports == 0 {
            "No MIDI ports connected · built-in piano active".into()
        } else {
            format!("MIDI ready · {} port{}", self.midi_ports, if self.midi_ports == 1 { "" } else { "s" })
        };
    }

    pub fn pump_midi(&mut self) {
        if !self.midi_ready {
            return;
        }
        for _ in 0..128 {
            let Some((_port, data)) = self.midi_input.receive() else { break };
            let status = data.data[0] & 0xf0;
            let key = data.data[1].min(127);
            let velocity = data.data[2].min(127);
            if status == 0x90 && velocity > 0 {
                self.playback.audition(0, &[key]);
                self.ui.shadow_pitch = Some(key);
                self.ui.status = format!("MIDI input · note {}", key);
            } else if status == 0x80 || status == 0x90 {
                self.playback.release_audition();
                self.ui.shadow_pitch = None;
            }
        }
    }

    /// What the transport is actually doing, straight from the published
    /// audio clock rather than the UI's own intent.
    pub fn engine_status(&self) -> &'static str {
        match self.playback.clock_snapshot().state {
            PlaybackState::Playing => "Playing",
            PlaybackState::Paused => "Paused",
            PlaybackState::Stopped => "Stopped",
        }
    }

    pub fn key_and_meter(&self) -> String {
        self.document.key_and_meter()
    }

    pub fn midi_status(&self) -> String {
        if self.midi_ports == 0 {
            "Built-in piano".into()
        } else {
            format!("Built-in piano + {} MIDI", self.midi_ports)
        }
    }

    pub fn handle_canvas_tap(&mut self, semantic: SemanticId, extend: bool) {
        if self.ui.annotation_tool != AnnotationTool::None {
            match self.ui.annotation_tool {
                AnnotationTool::Text | AnnotationTool::Fingering => {
                    self.ui.pending_annotation_target = Some(semantic);
                    self.ui.dialog = DialogKind::AnnotationText;
                }
                tool => {
                    if let Some(kind) = tool.kind() {
                        let result = self.document.add_annotation(semantic, kind, None);
                        self.report(result);
                    }
                }
            }
            return;
        }
        if extend {
            let all = self.document.all_note_semantics();
            let from = self.ui.selection.active.and_then(|active| all.iter().position(|id| *id == active));
            let to = all.iter().position(|id| *id == semantic);
            if let (Some(from), Some(to)) = (from, to) {
                let (start, end) = if from <= to { (from, to) } else { (to, from) };
                self.ui.selection.ordered = all[start..=end].to_vec();
                self.ui.selection.active = Some(semantic);
            } else {
                self.ui.selection.replace(semantic);
            }
        } else {
            self.ui.selection.replace(semantic);
        }
        self.ui.caret = Some(semantic);
        self.ui.status = self.selection_description();
    }

    /// What a rubber band swept up. Written straight rather than through
    /// [`Self::handle_canvas_tap`], because a band replaces the whole
    /// selection every pointer sample and must not audition sixty notes a
    /// second while it does.
    pub fn set_band_selection(&mut self, chosen: &[SemanticId]) {
        if self.ui.selection.ordered == chosen {
            return;
        }
        self.ui.selection.ordered = chosen.to_vec();
        self.ui.selection.active = chosen.last().copied();
        self.ui.caret = self.ui.selection.active;
        self.ui.status = self.selection_description();
    }

    pub fn handle_ink(&mut self, semantic: SemanticId, points: &[makepad_score_render::Point]) {
        let result = self.document.add_ink_annotation(semantic, points);
        self.report(result);
    }

    pub fn handle_mouse_entry(
        &mut self,
        cx: &mut Cx,
        measure: SemanticId,
        midi: u8,
        horizontal_fraction: f64,
    ) {
        match self
            .document
            .enter_note(measure, midi, horizontal_fraction, self.ui.entry_duration)
        {
            Ok(semantic) => {
                self.ui.selection.replace(semantic);
                self.ui.caret = Some(semantic);
                self.ui.status = format!("Entered MIDI {midi}");
                self.rebuild_playback(cx);
            }
            Err(error) => self.ui.status = error.to_string(),
        }
    }

    /// Live feedback while a note is being dragged: the new pitch sounds the
    /// moment the pointer crosses a staff step, and the status bar says what
    /// dropping here would do — or why it would be refused.
    pub fn preview_note_drag(&mut self, drag: &NoteDrag, target: &DragTarget, copy: bool) {
        self.ui.shadow_pitch = Some(target.midi);
        self.ui.status = drag_description(&self.document, drag, target, copy);
        if target.problem.is_none() {
            self.audition_pitch(target.midi);
        } else {
            self.playback.release_audition();
            self.release_midi_hover();
        }
    }

    /// Commits a drag as one undoable edit, or reports why it could not be.
    pub fn finish_note_drag(
        &mut self,
        cx: &mut Cx,
        drag: &NoteDrag,
        target: &DragTarget,
        copy: bool,
    ) {
        match self.document.commit_note_drag(drag, target, copy) {
            Ok(semantic) => {
                self.ui.selection.replace(semantic);
                self.ui.caret = Some(semantic);
                self.ui.status = drag_description(&self.document, drag, target, copy);
                self.rebuild_playback(cx);
            }
            Err(error) => self.ui.status = format!("Drop refused · {error}"),
        }
        self.ui.shadow_pitch = None;
        self.release_hover();
    }

    /// True while the piece itself is sounding, read from the published audio
    /// clock rather than the transport button's own flag: the two have drifted
    /// apart before, and the pointer must not play notes over the music.
    pub fn engine_playing(&self) -> bool {
        self.playback.clock_snapshot().state == PlaybackState::Playing
    }

    /// Whether pointing at a note should sound it. The preference is the
    /// user's setting; playing is the rule on top of it.
    pub fn hover_audition_allowed(&self) -> bool {
        self.prefs.audition_on_hover && !self.engine_playing()
    }

    /// The instrument a startup preset describes can only be built once the
    /// audio device has reported its rate — a preset with a design override is
    /// a different instrument, not a setting, and one built at the wrong rate
    /// is the wrong piano. Called once a frame; two relaxed loads when idle.
    fn settle_instrument(&mut self) {
        let Some(rate) = self.playback.device_rate() else {
            return;
        };
        if self.instrument_rate == Some(rate) {
            return;
        }
        self.instrument_rate = Some(rate);
        // The learned engine has to be built whatever the preset: unlike the
        // physical model there is no "default instrument" already standing.
        if self.sound.engine != ScoreEngine::Physical {
            self.playback.rebuild_instrument(
                self.sound.engine,
                sound::build_preset(self.sound.engine, self.sound.preset),
            );
        }
    }

    /// A note the pointer was sounding when the transport started is released
    /// rather than left ringing under the music.
    fn settle_hover_audition(&mut self) {
        if self.hover_sounding && self.engine_playing() {
            self.hover_sounding = false;
            self.playback.release_audition();
            self.release_midi_hover();
        }
    }

    pub fn audition_pitch(&mut self, midi: u8) {
        self.release_midi_hover();
        self.playback.audition(0, &[midi]);
        if self.midi_ready {
            self.midi_output.send(
                None,
                MidiNote {
                    is_on: true,
                    channel: 0,
                    note_number: midi,
                    velocity: 86,
                }
                .into(),
            );
            self.midi_hover = vec![midi];
        }
    }

    /// The pointer moving over an element. The highlight is always taken —
    /// seeing what you are pointing at is reading, and costs nothing — while
    /// the *sound* is a separate decision: the user's hover-audition setting,
    /// and never over a piece that is playing.
    pub fn audition_semantic(&mut self, semantic: Option<SemanticId>) {
        if self.ui.hover == semantic {
            return;
        }
        self.release_midi_hover();
        self.ui.hover = semantic;
        let pitches = semantic
            .and_then(|id| self.document.element(id))
            .and_then(|element| element.midi)
            .map(|pitch| vec![pitch])
            .unwrap_or_default();
        if pitches.is_empty() || !self.hover_audition_allowed() {
            if self.hover_sounding {
                self.hover_sounding = false;
                self.playback.release_audition();
            }
            return;
        }
        self.hover_sounding = true;
        self.playback.audition(0, &pitches);
        if self.midi_ready && !self.midi_muted {
            for pitch in &pitches {
                self.midi_output.send(
                    None,
                    MidiNote {
                        is_on: true,
                        channel: 0,
                        note_number: *pitch,
                        velocity: 86,
                    }
                    .into(),
                );
            }
            self.midi_hover = pitches;
        }
    }

    /// The hover reading aid marks *what sounds*. A bar's full-height hit
    /// rect is hovered constantly while reading and carries no pitch, so a
    /// wash over it would be a permanent stripe across the page.
    pub fn hovered_sounding(&self) -> Option<SemanticId> {
        let semantic = self.ui.hover?;
        self.document.element(semantic)?.midi?;
        Some(semantic)
    }

    pub fn release_hover(&mut self) {
        self.ui.hover = None;
        self.hover_sounding = false;
        self.playback.release_audition();
        self.release_midi_hover();
    }

    fn release_midi_hover(&mut self) {
        if self.midi_ready && !self.midi_muted {
            for pitch in self.midi_hover.drain(..) {
                self.midi_output.send(
                    None,
                    MidiNote {
                        is_on: false,
                        channel: 0,
                        note_number: pitch,
                        velocity: 0,
                    }
                    .into(),
                );
            }
        } else {
            self.midi_hover.clear();
        }
    }

    pub fn scrub_semantic(&mut self, semantic: SemanticId, speed: f32) {
        if let Some(element) = self.document.element(semantic) {
            if let Some(pitch) = element.midi {
                self.playback.scrub(semantic.0, 0, &[pitch], speed);
            }
        }
    }

    pub fn scrub_quarter(&mut self, quarter: f64, speed: f32) {
        if let Some(semantic) = self.document.semantic_near_quarter(quarter) {
            self.scrub_semantic(semantic, speed);
        }
    }

    pub fn playback_overlay(&self) -> (Option<PlaybackPosition>, Option<SemanticId>, f64) {
        let display = self.playback.display_position();
        let quarter = display.score_quarter.max(0.0);
        let whole = quarter / 4.0;
        let measure = self.document.score().measures.values().find(|measure| {
            let start = rational_f64(measure.start.0);
            let end = start + rational_f64(measure.extent.0);
            whole >= start && whole < end
        });
        let Some(measure) = measure else { return (None, None, quarter) };
        // The cursor rides the engraved column positions, not a nominal grid.
        let Some(location) = self.document.locate(whole) else {
            return (None, self.document.measure_semantic(measure.id), quarter);
        };
        (
            Some(PlaybackPosition {
                page: PageId(location.page as u32),
                x_sp: location.x_sp,
                system_span_sp: Some((location.top_sp, location.bottom_sp)),
            }),
            self.document.measure_semantic(measure.id),
            quarter,
        )
    }

    pub fn sync_follow_page(&mut self) {
        // Hand over any instrument a rebuild preset asked for, and drop the
        // one the audio thread finished with. Two relaxed loads when idle.
        self.playback.service_instrument();
        self.settle_instrument();
        self.settle_hover_audition();
        self.playback
            .service_metronome(self.practice.metronome, self.practice.tempo);
        let engine_playing = self.playback.clock_snapshot().state == PlaybackState::Playing;
        if self
            .practice
            .sync_transport(engine_playing, Cx::time_now())
            && !self.practice.playing
        {
            self.ui.status = "Reached the end".into();
        }
        // A reader who is zooming has taken hold of the view. The follow
        // would re-centre the page under them on the very frames the zoom is
        // holding a point still, and the two writing `pan` in turn is the
        // wobble that only ever showed up during playback.
        if !self.practice.follow_cursor || !self.practice.playing || self.ui.zooming {
            return;
        }
        let (position, _, _) = self.playback_overlay();
        if let Some(position) = position {
            let page = (position.page.0 as usize)
                .min(self.document.page_count().saturating_sub(1));
            if page != self.ui.current_page {
                self.ui.go_to_page(page);
            }
        }
    }

    pub fn selection_description(&self) -> String {
        let count = self.ui.selection.ordered.len();
        let Some(active) = self.ui.selection.active.and_then(|id| self.document.element(id)) else {
            return "No selection".into();
        };
        match active.midi {
            Some(midi) if count == 1 => format!("Note · MIDI {} · bar {}", midi, self.document.score().measures[&active.measure].label),
            _ => format!("{} elements selected", count),
        }
    }

    pub fn history_lines(&self) -> Vec<String> {
        self.document
            .workspace()
            .journal()
            .iter()
            .rev()
            .take(5)
            .map(|transaction| {
                let action = if transaction.undoes.is_some() { "Undo" } else { "Edit" };
                format!("{} {:04} · {} change{}", action, transaction.id.counter, transaction.ops.len(), if transaction.ops.len() == 1 { "" } else { "s" })
            })
            .collect()
    }

    fn report(&mut self, result: Result<(), DocumentError>) {
        self.ui.status = match result {
            Ok(()) => "Score updated".into(),
            Err(error) => error.to_string(),
        };
    }
}

/// One line saying what this drag is doing, for the status bar. It names the
/// pitch it would land on, the beat it would land on when the drag is
/// horizontal, and the reason when the drop would be refused.
pub fn drag_description(
    document: &ScoreDocument,
    drag: &NoteDrag,
    target: &DragTarget,
    copy: bool,
) -> String {
    if let Some(problem) = target.problem {
        return format!("Cannot drop here · {problem}");
    }
    let verb = if copy { "Copy" } else { "Drag" };
    let bar = document
        .score()
        .measures
        .get(&drag.measure)
        .map(|measure| measure.label.clone())
        .unwrap_or_default();
    let mut line = format!(
        "{verb} · {} → {} · bar {bar}",
        pitch_name(drag.diatonic, drag.alter),
        pitch_name_of(target.pitch),
    );
    if target.onset != drag.onset {
        line.push_str(&format!(" · beat {:.2}", document.beat_in_measure(drag, target)));
    }
    line
}

fn pitch_name_of(pitch: makepad_score::model::Pitch) -> String {
    pitch_name(
        i32::from(pitch.octave) * 7 + i32::from(pitch.step.index()),
        (pitch.alter.0.numerator() as f64 / pitch.alter.0.denominator() as f64).round() as i32,
    )
}

fn pitch_name(diatonic: i32, alter: i32) -> String {
    const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];
    let letter = LETTERS[diatonic.rem_euclid(7) as usize];
    let accidental = match alter {
        -2 => "bb",
        -1 => "b",
        0 => "",
        1 => "#",
        _ => "##",
    };
    format!("{letter}{accidental}{}", diatonic.div_euclid(7))
}

/// The score's own opening tempo, when it carries one. An imported performance
/// knows its tempo; playing it at the app default is simply wrong music.
fn score_opening_tempo(score: &makepad_score::model::Score) -> Option<f64> {
    score.maps.tempo.iter().find_map(|change| match change.value {
        makepad_score::model::Tempo::Instant { quarters_per_minute } => {
            let bpm = rational_f64(quarters_per_minute);
            (bpm.is_finite() && bpm >= 20.0 && bpm <= 400.0).then_some(bpm)
        }
        _ => None,
    })
}

/// How many library entries one page of the browser lists.
pub const LIBRARY_PAGE: usize = 16;

impl ScoreAppState {
    /// Hand the whole sound to the audio thread. One release store; the synth
    /// itself is never touched from here.
    pub fn publish_sound(&mut self) {
        self.playback.set_sound(self.sound);
    }

    /// The last page index the current library has entries for.
    pub fn library_last_page(&self) -> usize {
        self.library
            .entries()
            .len()
            .saturating_sub(1)
            / LIBRARY_PAGE
    }

    /// The slice of entries the browser is showing.
    pub fn library_visible(&self) -> &[crate::library::LibraryEntry] {
        let entries = self.library.entries();
        let start = (self.library_page * LIBRARY_PAGE).min(entries.len());
        let end = (start + LIBRARY_PAGE).min(entries.len());
        &entries[start..end]
    }

    fn adopt_score_tempo(&mut self) {
        if let Some(bpm) = score_opening_tempo(self.document.score()) {
            self.practice.tempo = bpm;
        }
    }

    fn rebuild_playback(&mut self, cx: &mut Cx) {
        self.playback
            .rebuild_plan(self.document.score(), self.practice.tempo, self.practice.count_in);
        self.playback.install_audio_output(cx);
    }

    /// Which instrument is playing.
    ///
    /// Derived from the engine rather than stored beside it, so the list's
    /// highlight cannot drift out of step with what is actually sounding.
    pub fn selected_instrument(&self) -> InstrumentId {
        match self.sound.engine {
            ScoreEngine::Learned => InstrumentId::Electric(self.sound.preset),
            _ => InstrumentId::Acoustic(self.sound.preset),
        }
    }

    /// Pick an instrument. Routing to the right synthesis is this function's
    /// job, not the reader's: that is the whole point of one flat list.
    pub fn select_instrument(&mut self, id: InstrumentId) {
        let (InstrumentId::Acoustic(index) | InstrumentId::Electric(index)) = id;
        let engine = id.engine();
        self.sound.engine = engine;
        self.sound.apply_preset(index);
        self.ui.sound_focus = None;
        // Every instrument is BUILT, the physical one included: its 88 key
        // designs and modal banks are constructed from the preset just as the
        // learned engine's network is. Skipping the build for one engine
        // leaves the other one installed and still sounding, so the list moves
        // and the ear does not. The handoff is crossfaded, so switching
        // mid-phrase dissolves rather than cuts.
        self.playback
            .rebuild_instrument(engine, sound::build_preset(engine, index));
        self.publish_sound();
        // The instrument is a choice, so it outlives the session.
        self.prefs.engine = crate::prefs::engine_name(engine).to_string();
        self.prefs.instrument = sound::preset_name(engine, index).to_string();
        self.prefs.save();
        self.ui.status = format!(
            "{} · {}",
            sound::preset_name(engine, index),
            sound::preset_description(engine, index)
        );
    }

    /// Store the two exposed controls, so the room and the voicing the reader
    /// settled on are still there next launch.
    pub fn remember_sound(&mut self) {
        self.prefs.room = crate::prefs::reverb_preset_name(self.sound.room.preset).to_string();
        self.prefs.reverb = Some(self.sound.room.mix);
        self.prefs.brightness = self.sound.eq_shelf_db;
        self.prefs.save();
    }
}

pub fn apply_score_action(cx: &mut Cx, state: &mut ScoreAppState, action: &ScoreAction) -> bool {
    match action {
        ScoreAction::SetMode(mode) => {
            state.ui.mode = *mode;
            state.ui.chrome_visible = *mode == ProductMode::Editor;
            if *mode == ProductMode::Pianist {
                state.ui.tool = ScoreTool::Navigate;
            }
            state.ui.status = match mode {
                ProductMode::Pianist => "Pianist mode · score only".into(),
                ProductMode::Editor => "Editor mode · notation tools revealed".into(),
            };
        }
        ScoreAction::SetTool(tool) => {
            // Pianist mode is the reading face: it navigates and never edits,
            // so arming an editing tool reveals the editor rather than quietly
            // making the reading face dangerous.
            if *tool != ScoreTool::Navigate && state.ui.mode == ProductMode::Pianist {
                state.ui.mode = ProductMode::Editor;
                state.ui.chrome_visible = true;
            }
            state.ui.tool = *tool;
            state.ui.status = tool.hint().into();
        }
        ScoreAction::Transpose(semitones) => {
            let selection = state.ui.selection.ordered.clone();
            if selection.is_empty() {
                state.ui.status = "Nothing selected to transpose".into();
            } else {
                match state.document.transpose(&selection, *semitones) {
                    Ok(moved) => {
                        state.ui.status = format!(
                            "Transposed {} note{} by {}{}",
                            moved,
                            if moved == 1 { "" } else { "s" },
                            if *semitones > 0 { "+" } else { "" },
                            semitones
                        );
                        state.rebuild_playback(cx);
                    }
                    Err(error) => state.ui.status = format!("Transpose refused · {error}"),
                }
            }
        }
        ScoreAction::DeleteSelection => {
            let selection = state.ui.selection.ordered.clone();
            if selection.is_empty() {
                state.ui.status = "Nothing selected to delete".into();
            } else {
                match state.document.delete_notes(&selection) {
                    Ok(removed) => {
                        state.ui.selection.clear();
                        state.ui.caret = None;
                        state.ui.status = format!(
                            "Deleted {} note{}",
                            removed,
                            if removed == 1 { "" } else { "s" }
                        );
                        state.rebuild_playback(cx);
                    }
                    Err(error) => state.ui.status = format!("Delete refused · {error}"),
                }
            }
        }
        ScoreAction::ToggleMode | ScoreAction::ToggleChrome => {
            state.ui.mode = if state.ui.mode == ProductMode::Pianist {
                ProductMode::Editor
            } else {
                ProductMode::Pianist
            };
            state.ui.chrome_visible = state.ui.mode == ProductMode::Editor;
        }
        ScoreAction::SetPageLayout(layout) => {
            state.ui.page_layout = *layout;
            state.ui.recentre_now();
            state.ui.status = format!("{} layout", layout.label());
        }
        ScoreAction::PageDelta(delta) => {
            let count = state.document.page_count();
            if count > 0 {
                let from = state.ui.current_page;
                let to = (from as i64 + i64::from(*delta)).clamp(0, count as i64 - 1) as usize;
                if from != to {
                    state.ui.go_to_page(to);
                }
            }
        }
        ScoreAction::FirstPage => state.ui.go_to_page(0),
        ScoreAction::LastPage => {
            state.ui.go_to_page(state.document.page_count().saturating_sub(1))
        }
        ScoreAction::ZoomBy(factor) => {
            // Zoom from a menu or a key has no pointer to zoom about, so it
            // keeps the page it is on centred; the canvas zooms about the
            // pointer for the wheel.
            state.ui.zoom = (state.ui.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            state.ui.go_to_page(state.ui.current_page);
            state.ui.glide.active = true;
            state.ui.status = format!("Zoom {}%", (state.ui.zoom * 100.0).round());
        }
        ScoreAction::FitAllPages => {
            // The canvas owns the geometry, so it works out the zoom that
            // holds the whole document and recentres on the next frame.
            state.ui.fit_all = true;
            state.ui.status = "Whole document".into();
        }
        ScoreAction::FitPage => {
            state.ui.zoom = 1.0;
            state.ui.recentre_now();
            state.ui.status = "Fit page".into();
        }
        ScoreAction::RevealControls(visible) => {
            if *visible {
                state.ui.reveal_controls(Cx::time_now());
            } else {
                state.ui.controls_visible = false;
                state.ui.controls_pinned = false;
            }
        }
        ScoreAction::PlayPause => {
            state.practice.playing = !state.practice.playing;
            state.practice.play_requested_at = Cx::time_now();
            if state.practice.playing {
                state.playback.play();
                state.ui.status = "Playing · audio clock master".into();
            } else {
                state.playback.pause();
                state.ui.status = "Paused".into();
            }
        }
        ScoreAction::Stop => {
            state.practice.playing = false;
            state.practice.play_requested_at = Cx::time_now();
            state.playback.stop();
            state.ui.status = "Stopped".into();
        }
        ScoreAction::ToggleMetronome => {
            state.practice.metronome = !state.practice.metronome;
            state.ui.status = if state.practice.metronome { "Metronome on" } else { "Metronome off" }.into();
        }
        ScoreAction::ToggleCountIn => {
            state.practice.count_in = !state.practice.count_in;
            state.rebuild_playback(cx);
            state.ui.status = if state.practice.count_in {
                "Count-in on · one bar before the music"
            } else {
                "Count-in off"
            }
            .into();
        }
        ScoreAction::ToggleFollow => {
            state.practice.follow_cursor = !state.practice.follow_cursor;
            state.ui.status = if state.practice.follow_cursor {
                "Following the playback cursor"
            } else {
                "Page stays put during playback"
            }
            .into();
        }
        ScoreAction::ToggleLoop => {
            state.practice.loop_enabled = !state.practice.loop_enabled;
            if state.practice.loop_enabled {
                if let Some((start, end)) = state
                    .document
                    .loop_range_for_selection(&state.ui.selection.ordered)
                {
                    state.practice.loop_start_quarter = start;
                    state.practice.loop_end_quarter = end;
                }
            }
            state.playback.set_loop(
                state.practice.loop_start_quarter,
                state.practice.loop_end_quarter,
                state.practice.loop_enabled,
            );
            state.ui.status = if state.practice.loop_enabled {
                format!(
                    "Practice loop · quarters {:.0}–{:.0}",
                    state.practice.loop_start_quarter, state.practice.loop_end_quarter
                )
            } else {
                "Practice loop off".into()
            };
        }
        ScoreAction::SetTempo(bpm) => {
            state.practice.tempo = bpm.clamp(20.0, 400.0);
            state.playback.set_tempo(state.practice.tempo);
        }
        ScoreAction::SeekQuarter(quarter) => state.playback.seek_quarter(*quarter),
        ScoreAction::SetReverbPreset(preset) => {
            state.sound.room.preset = *preset;
            state.publish_sound();
            state.remember_sound();
            state.ui.status = format!("Room · {}", crate::playback::reverb_preset_label(*preset));
        }
        ScoreAction::SelectInstrument(id) => state.select_instrument(*id),
        ScoreAction::SetSoundParam { param, value } => {
            param.set(&mut state.sound, *value);
            state.ui.sound_focus = Some(*param);
            state.publish_sound();
            state.remember_sound();
            state.ui.status = format!("{} · {}", param.label(), param.format(param.get(&state.sound)));
        }
        ScoreAction::SetAnnotationTool(tool) => {
            state.ui.annotation_tool = *tool;
            state.ui.status = if *tool == AnnotationTool::None {
                "Annotation tool closed".into()
            } else {
                format!("{:?} annotation · tap a note", tool)
            };
        }
        ScoreAction::ApplyAnnotationText(text) => {
            if let Some(target) = state.ui.pending_annotation_target.take() {
                let kind = if state.ui.annotation_tool == AnnotationTool::Fingering {
                    AnnotationKind::Fingering
                } else {
                    AnnotationKind::Text
                };
                let result = state.document.add_annotation(target, kind, Some(text.clone()));
                state.report(result);
            }
            state.ui.dialog = DialogKind::None;
        }
        ScoreAction::SetInspectorTab(tab) => {
            state.ui.inspector_tab = *tab;
            state.ui.status = format!("{:?} inspector", tab);
        }
        ScoreAction::SelectMore => {
            let expanded = state.document.select_more(&state.ui.selection.ordered);
            if !expanded.is_empty() {
                state.ui.selection.ordered = expanded;
                state.ui.selection.active = state.ui.selection.ordered.last().copied();
            }
            state.ui.status = state.selection_description();
        }
        ScoreAction::SelectAll => {
            state.ui.selection.ordered = state.document.all_note_semantics();
            state.ui.selection.active = state.ui.selection.ordered.last().copied();
            state.ui.status = state.selection_description();
        }
        ScoreAction::ClearSelection => {
            state.ui.selection.clear();
            state.ui.status = "Selection cleared".into();
        }
        ScoreAction::Undo => {
            let result = state.document.undo();
            state.report(result);
            state.rebuild_playback(cx);
        }
        ScoreAction::Redo => {
            let result = state.document.redo();
            state.report(result);
            state.rebuild_playback(cx);
        }
        ScoreAction::SetDuration(key) => {
            state.ui.entry_duration = *key;
            if let Some(target) = state.ui.selection.active.or(state.ui.caret) {
                let result = state.document.set_duration(target, *key);
                state.report(result);
                state.rebuild_playback(cx);
            }
        }
        ScoreAction::EnterPitch(letter) => {
            if let Some(target) = state.ui.caret.or(state.ui.selection.active) {
                let result = state.document.change_pitch(target, *letter);
                let next = state.document.next_note_semantic(target);
                state.report(result);
                state.ui.caret = next.or(Some(target));
                if let Some(next) = state.ui.caret {
                    state.ui.selection.replace(next);
                }
                state.rebuild_playback(cx);
            } else {
                state.ui.status = "Click a note to place the entry caret".into();
            }
        }
        ScoreAction::ApplyPalette(command) => {
            let Some(target) = state.ui.selection.active else {
                state.ui.status = "Select a note first".into();
                return true;
            };
            let articulation = match command {
                PaletteCommand::Staccato => Some(Articulation::Staccato),
                PaletteCommand::Accent => Some(Articulation::Accent),
                PaletteCommand::Tenuto => Some(Articulation::Tenuto),
                _ => None,
            };
            if let Some(articulation) = articulation {
                let result = state.document.apply_articulation(target, articulation);
                state.report(result);
            } else {
                // Accidentals are a written-pitch edit, not a decoration: the
                // engraver draws them straight off the note's alteration.
                let alter = match command {
                    PaletteCommand::Sharp => 1,
                    PaletteCommand::Flat => -1,
                    _ => 0,
                };
                let result = state.document.set_alter(target, alter);
                state.report(result);
                state.rebuild_playback(cx);
            }
        }
        ScoreAction::SetPartGain { part, delta } => {
            if let Some(channel) = state.parts.get_mut(*part) {
                channel.gain = (channel.gain + delta).clamp(0.0, 1.25);
                state.playback.set_part_mix(*part, channel.gain, channel.pan, channel.mute, channel.solo);
            }
        }
        ScoreAction::SetPartPan { part, delta } => {
            if let Some(channel) = state.parts.get_mut(*part) {
                channel.pan = (channel.pan + delta).clamp(-1.0, 1.0);
                state.playback.set_part_mix(*part, channel.gain, channel.pan, channel.mute, channel.solo);
            }
        }
        ScoreAction::TogglePartMute(part) => {
            if let Some(channel) = state.parts.get_mut(*part) {
                channel.mute = !channel.mute;
                state.playback.set_part_mix(*part, channel.gain, channel.pan, channel.mute, channel.solo);
            }
        }
        ScoreAction::TogglePartSolo(part) => {
            if let Some(channel) = state.parts.get_mut(*part) {
                channel.solo = !channel.solo;
                state.playback.set_part_mix(*part, channel.gain, channel.pan, channel.mute, channel.solo);
            }
        }
        ScoreAction::OpenDialog(dialog) => {
            state.ui.dialog = *dialog;
            state.ui.dialog_error = None;
            if *dialog == DialogKind::Library {
                // The folder is read here, not at launch: a browser nobody
                // opens costs nothing.
                state.library.ensure_scanned();
                // Open on the page the piece being read is on.
                state.library_page = state
                    .document
                    .path()
                    .and_then(|path| state.library.index_of(path))
                    .map(|index| index / LIBRARY_PAGE)
                    .unwrap_or(state.library_page)
                    .min(state.library_last_page());
            }
            state.ui.draft = DialogDraft {
                layout: state.ui.page_layout,
                zoom: state.ui.zoom,
                tempo: state.practice.tempo,
            };
        }
        ScoreAction::CloseDialog => {
            state.ui.dialog = DialogKind::None;
            state.ui.dialog_error = None;
            state.ui.context_menu_at = None;
        }
        ScoreAction::Dismiss => {
            // Escape peels one layer at a time: dialog, then context menu,
            // then the selection. The Edit menu promises exactly this.
            if state.ui.dialog != DialogKind::None {
                state.ui.dialog = DialogKind::None;
                state.ui.dialog_error = None;
            } else if state.ui.context_menu_at.is_some() {
                state.ui.context_menu_at = None;
            } else if state.ui.annotation_tool != AnnotationTool::None {
                state.ui.annotation_tool = AnnotationTool::None;
                state.ui.status = "Annotation tool closed".into();
            } else if state.ui.tool != ScoreTool::Navigate {
                // Escape always ends at the tool that cannot change anything.
                state.ui.tool = ScoreTool::Navigate;
                state.ui.status = ScoreTool::Navigate.hint().into();
            } else {
                state.ui.selection.clear();
                state.ui.status = "Selection cleared".into();
            }
        }
        ScoreAction::TogglePref(toggle) => {
            match toggle {
                PrefToggle::StartInEditor => {
                    state.prefs.start_in_editor = !state.prefs.start_in_editor
                }
                PrefToggle::AuditionOnHover => {
                    state.prefs.audition_on_hover = !state.prefs.audition_on_hover;
                    if !state.prefs.audition_on_hover {
                        state.release_hover();
                    }
                }
                PrefToggle::FollowCursor => {
                    state.prefs.follow_cursor = !state.prefs.follow_cursor;
                    state.practice.follow_cursor = state.prefs.follow_cursor;
                }
                PrefToggle::Metronome => {
                    state.prefs.metronome = !state.prefs.metronome;
                    state.practice.metronome = state.prefs.metronome;
                }
                PrefToggle::CountIn => {
                    state.prefs.count_in = !state.prefs.count_in;
                    state.practice.count_in = state.prefs.count_in;
                    state.rebuild_playback(cx);
                }
                PrefToggle::DarkPaper => state.prefs.dark_paper = !state.prefs.dark_paper,
            }
            state.ui.status = "Preference changed · Apply to keep it".into();
        }
        ScoreAction::ApplyPreferences => {
            state.ui.dialog = DialogKind::None;
            state.ui.status = if state.prefs.save() {
                match crate::prefs::ScorePrefs::path() {
                    Some(path) => format!("Preferences saved to {}", path.display()),
                    None => "Preferences saved".into(),
                }
            } else {
                "Could not write the preferences file".into()
            };
        }
        ScoreAction::SetDialogLayout(layout) => state.ui.draft.layout = *layout,
        ScoreAction::SetDialogZoom(zoom) => state.ui.draft.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX),
        ScoreAction::SetDialogTempo(tempo) => state.ui.draft.tempo = tempo.clamp(20.0, 400.0),
        ScoreAction::ApplyScoreSetup { tempo } => {
            state.practice.tempo = tempo.clamp(20.0, 400.0);
            state.playback.set_tempo(state.practice.tempo);
            state.ui.dialog = DialogKind::None;
            state.ui.status = format!("Tempo {:.0} BPM", state.practice.tempo);
        }
        ScoreAction::ApplyPageSetup { layout, zoom } => {
            state.ui.page_layout = *layout;
            state.ui.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
            state.ui.recentre_now();
            state.ui.dialog = DialogKind::None;
            state.ui.status = format!(
                "{} · staff {}%",
                layout.label(),
                (state.ui.zoom * 100.0).round()
            );
        }
        ScoreAction::Browse(target) => {
            let title = match target {
                BrowseTarget::Open => "Open a score",
                BrowseTarget::SaveDirectory => "Choose a folder to save into",
                BrowseTarget::LibraryDirectory => "Choose your music folder",
            };
            let mut dialog = FileDialog::new().set_title(title.to_string());
            let start = match target {
                BrowseTarget::LibraryDirectory => {
                    state.library.dir().map(std::path::Path::to_path_buf)
                }
                _ => state.prefs.last_dir.clone(),
            };
            if let Some(dir) = start {
                dialog = dialog.set_location(dir);
            }
            // The one panel this platform layer actually implements chooses a
            // file OR a folder, which covers both callers.
            cx.open_select_folder_dialog(dialog);
            state.ui.status = "Choose a file in the system panel…".into();
        }
        ScoreAction::OpenRecent(index) => {
            if let Some(path) = state.prefs.recent.get(*index).cloned() {
                return apply_score_action(cx, state, &ScoreAction::OpenPath(path));
            }
        }
        ScoreAction::OpenLibraryEntry(row) => {
            let index = state.library_page * LIBRARY_PAGE + row;
            if let Some(entry) = state.library.entries().get(index) {
                let line = entry.line();
                let opened = match entry.source.clone() {
                    // A piece the binary carries opens from its own bytes;
                    // there is no file to name.
                    crate::library::EntrySource::Bundled {
                        bytes,
                        extension,
                        credit,
                        attribution,
                    } => {
                        let title = entry.title.clone();
                        let opened =
                            state.open_bundled_score(cx, bytes, extension, &title, credit);
                        state.performance_credit = attribution;
                        opened
                    }
                    crate::library::EntrySource::File => {
                        let path = entry.path.clone();
                        apply_score_action(cx, state, &ScoreAction::OpenPath(path))
                    }
                };
                // Say what was picked, not what the file titles itself — and
                // who played it, when somebody did.
                if state.ui.dialog_error.is_none() {
                    state.ui.status = match state.performance_credit {
                        Some(credit) => format!("{line}   ·   {credit}"),
                        None => line,
                    };
                }
                return opened;
            }
        }
        ScoreAction::SetLibraryDir(dir) => {
            state.library.set_dir(dir.clone());
            state.library_page = 0;
            state.prefs.library_dir = state.library.dir().map(PathBuf::from);
            state.prefs.save();
            state.ui.status = format!("Music library · {}", state.library.summary());
        }
        ScoreAction::RescanLibrary => {
            state.library.rescan();
            state.library_page = state.library_page.min(state.library_last_page());
            state.ui.status = format!("Music library · {}", state.library.summary());
        }
        ScoreAction::LibraryPage(delta) => {
            let last = state.library_last_page() as i64;
            state.library_page =
                (state.library_page as i64 + i64::from(*delta)).clamp(0, last) as usize;
        }
        ScoreAction::OpenPath(path) => match ScoreDocument::open(path.clone()) {
            Ok(document) => {
                state.document = document;
                state.parts = state.document.parts();
                state.ui.current_page = 0;
                state.ui.recentre_now();
                state.ui.selection.clear();
                state.ui.dialog = DialogKind::None;
                state.ui.dialog_error = None;
                state.prefs.remember(path);
                state.prefs.save();
                state.adopt_score_tempo();
                state.rebuild_playback(cx);
                // The file name, not the score's own title: an imported MIDI
                // often titles itself after its first track ("Piano right"),
                // which is not what the user just picked.
                state.ui.status = format!(
                    "Opened {}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                );
            }
            Err(error) => {
                let message = format!("{}: {error}", path.display());
                state.ui.dialog_error = Some(message.clone());
                state.ui.status = message;
            }
        },
        ScoreAction::SavePath(path) => {
            let path = crate::document::with_native_extension(path);
            match state.document.save(path.clone()) {
                Ok(()) => {
                    state.ui.dialog = DialogKind::None;
                    state.ui.dialog_error = None;
                    state.prefs.remember(&path);
                    state.prefs.save();
                    state.ui.status = format!("Saved {}", path.display());
                }
                Err(error) => {
                    let message = format!("{}: {error}", path.display());
                    state.ui.dialog_error = Some(message.clone());
                    state.ui.status = message;
                }
            }
        }
        ScoreAction::Save => {
            // An imported .mid or .musicxml is a source, not a save target:
            // writing the native workspace over it would destroy the import.
            match state.document.native_path().map(PathBuf::from) {
                Some(path) => {
                    let result = state.document.save(path.clone());
                    let ok = result.is_ok();
                    state.report(result);
                    if ok {
                        state.ui.status = format!("Saved {}", path.display());
                    }
                }
                None => {
                    state.ui.dialog = DialogKind::SaveAs;
                    state.ui.dialog_error = None;
                }
            }
        }
        ScoreAction::NewDemo => {
            if let Ok(document) = ScoreDocument::demo() {
                state.document = document;
                state.parts = state.document.parts();
                state.ui.current_page = 0;
                state.ui.recentre_now();
                state.ui.selection.clear();
                state.adopt_score_tempo();
                state.rebuild_playback(cx);
                state.ui.status = "New score".into();
            }
        }
        ScoreAction::Quit => cx.quit(),
        ScoreAction::ContextMenu { at, semantic } => {
            cx.clear_all_hovers();
            state.ui.context_menu_at = Some(*at);
            state.ui.context_semantic = semantic.map(SemanticId);
            if let Some(semantic) = state.ui.context_semantic {
                state.ui.selection.replace(semantic);
            }
        }
        ScoreAction::CloseContextMenu => {
            state.ui.context_menu_at = None;
            // The context menu opens and closes under the pointer, so the row
            // it leaves behind never gets its hover-out. Clear them.
            cx.clear_all_hovers();
        }
    }
    true
}

pub fn key_action(event: &KeyEvent, state: &ScoreAppState) -> Option<ScoreAction> {
    crate::keymap::action_for_key(
        event,
        state.ui.mode,
        state.ui.tool,
        state.ui.text_input_focused,
    )
}

pub fn transport_label(state: &ScoreAppState) -> &'static str {
    if state.practice.playing {
        "Pause"
    } else {
        "Play"
    }
}

fn rational_f64(value: makepad_score::model::Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported bug: the controls flickered up on movement and vanished
    /// again before they could be clicked. Movement must buy a real dwell, and
    /// the pointer resting on the strip must hold them open indefinitely.
    #[test]
    fn revealed_controls_dwell_long_enough_to_be_clicked() {
        let mut ui = ScoreUiState::default();
        assert!(!ui.controls_visible);
        assert!(ui.reveal_controls(10.0));
        assert!(ui.controls_visible);
        // Half a second later they are still up.
        assert!(!ui.tick_controls(10.5));
        assert!(ui.controls_visible);
        // Still up just before the dwell expires.
        assert!(!ui.tick_controls(10.0 + CONTROLS_DWELL_S - 0.05));
        assert!(ui.controls_visible);
        assert!(ui.tick_controls(10.0 + CONTROLS_DWELL_S));
        assert!(!ui.controls_visible);
        assert!(CONTROLS_DWELL_S >= 2.0, "a reader needs seconds, not frames");
    }

    #[test]
    fn controls_never_fade_while_the_pointer_is_on_them() {
        let mut ui = ScoreUiState::default();
        ui.reveal_controls(0.0);
        ui.controls_pinned = true;
        for step in 0..100 {
            assert!(!ui.tick_controls(step as f64));
            assert!(ui.controls_visible);
        }
        ui.controls_pinned = false;
        assert!(ui.tick_controls(99.0 + CONTROLS_DWELL_S));
        assert!(!ui.controls_visible);
    }

    /// The engine stops at the end of the piece. Before this the UI kept its
    /// own `playing` flag, so the button still read "Pause": the next press
    /// sent Pause to a stopped engine and it took two presses to play again.
    #[test]
    fn the_transport_flag_follows_the_engine_once_the_request_has_landed() {
        let mut practice = PracticeState::default();
        practice.playing = true;
        practice.play_requested_at = 10.0;
        // Inside the grace window the engine has not answered yet.
        assert!(!practice.sync_transport(false, 10.0 + TRANSPORT_GRACE_S * 0.5));
        assert!(practice.playing);
        // After it, a stopped engine wins.
        assert!(practice.sync_transport(false, 10.0 + TRANSPORT_GRACE_S));
        assert!(!practice.playing);
        // And a transport started elsewhere lights the button up again.
        assert!(practice.sync_transport(true, 20.0));
        assert!(practice.playing);
        assert!(!practice.sync_transport(true, 21.0));
    }

    /// The application opens in the editor, because that is what the shipped
    /// preference says — the mode is a default, not a hard-coded face, so a
    /// reader who chose pianist mode still gets it.
    #[test]
    fn the_opening_face_follows_the_preference() {
        let editor = ScoreUiState::from_prefs(&ScorePrefs {
            start_in_editor: true,
            ..ScorePrefs::default()
        });
        assert_eq!(editor.mode, ProductMode::Editor);
        assert!(editor.chrome_visible);
        let pianist = ScoreUiState::from_prefs(&ScorePrefs {
            start_in_editor: false,
            ..ScorePrefs::default()
        });
        assert_eq!(pianist.mode, ProductMode::Pianist);
        assert!(!pianist.chrome_visible);
        assert!(!pianist.controls_visible);
        // Shipped: the editor, the strip of pages, no scroll offset.
        assert!(ScorePrefs::default().start_in_editor);
        assert_eq!(ScoreUiState::default().page_layout, PageLayout::Single);
        assert_eq!(ScoreUiState::default().pan, DVec2::default());
    }

    /// Pointing at a note sounds it while the score is stopped and stays
    /// silent while the piece is playing — the halo is still taken either way,
    /// because seeing what you are pointing at is reading, not playing.
    ///
    /// The rule reads the published audio clock, not the transport button's
    /// own flag: those two have drifted apart before.
    #[test]
    fn hover_audition_is_gated_by_the_transport_under_the_preference() {
        use makepad_score_play::AudioClockSnapshot;
        let mut state = ScoreAppState::default();
        state.prefs.audition_on_hover = true;
        assert!(!state.engine_playing(), "a fresh state is not playing");
        assert!(state.hover_audition_allowed());

        state.playback.publish_clock_for_test(AudioClockSnapshot {
            state: PlaybackState::Playing,
            ..AudioClockSnapshot::default()
        });
        assert!(state.engine_playing());
        assert!(!state.hover_audition_allowed(), "no notes over the music");

        // The pointer still marks what it is over while the piece plays.
        let semantic = state.document.all_note_semantics()[0];
        state.audition_semantic(Some(semantic));
        assert_eq!(state.ui.hover, Some(semantic), "the halo is still taken");

        // Pausing hands the pointer its voice back.
        state.playback.publish_clock_for_test(AudioClockSnapshot {
            state: PlaybackState::Paused,
            ..AudioClockSnapshot::default()
        });
        assert!(!state.engine_playing());
        assert!(state.hover_audition_allowed());

        state.prefs.audition_on_hover = false;
        assert!(!state.hover_audition_allowed(), "the reader's setting still rules");
    }
}
