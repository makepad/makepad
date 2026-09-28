//! The Material component's third page: what each of the knob engine's
//! detail levels costs, measured in Makepad itself.
//!
//! Each version (`crate::knob::lod`) is its own draw shader and is built
//! only when the reader asks: a version's Compile creates its knob widget
//! from its template, which is when the shader's text is generated and
//! queued for the backend, and the page polls once a frame until the
//! backend says the shader is ready. Measure draws a grid of the version's
//! knobs, redraws it every frame for a couple of seconds, and reads the
//! containing pass's GPU time (where the backend records it) and the frame
//! interval, against the same grid empty.
use crate::controls::{ControlValue, StoryControlAction};
use crate::knob::lod::{launch_salt, VERSIONS};
use crate::knob::look::{KnobLook, VALUE};
use crate::knob::presets::{KnobMaterial, MATERIALS, STYLES};
use crate::knob::sweep::{self, ComboRecord, CompileRecord, RunRecord, SweepReport};
use crate::knob::widgets::{set_material_uniforms, TurnedKnob, TurnedKnobAction};
use crate::makepad_widgets::makepad_script::trap::NoTrap;
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::collections::VecDeque;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::knob::script_mod(vm);
    page::script_mod(vm)
}

mod page {
    use super::{KnobCost, KnobGrid, KnobSlot};
    use crate::makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets.*
        use mod.widgets.*
        use mod.storybook.*

        mod.storybook.KnobSlotBase = #(KnobSlot::register_widget(vm))
        /** Where a version's knob stands: the knob once it is compiled,
         * a ring until then. */
        mod.storybook.KnobSlot = set_type_default() do mod.storybook.KnobSlotBase{
            width: 200.
            height: 200.
            interactive: true
            fill: 0.62
            draw_empty +: {
                color: uniform(vec4(0.5, 0.5, 0.5, 0.6))
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    let c = self.rect_size * 0.5
                    sdf.circle(c.x, c.y, min(c.x, c.y) * 0.62)
                    sdf.stroke(self.color, 1.5)
                    return sdf.result
                }
            }
        }

        mod.storybook.KnobGridBase = #(KnobGrid::register_widget(vm))
        /** The measuring grid: one knob drawn N times, or the same space
         * empty. */
        mod.storybook.KnobGrid = set_type_default() do mod.storybook.KnobGridBase{
            width: Fill
            height: Fit
            flow: Flow.Right{wrap: true}
            // A knob's light and shadow reach past its cell: nothing here
            // clips them.
            clip_x: false
            clip_y: false
            fill: 0.62
        }

        mod.storybook.KnobCostBase = #(KnobCost::register_widget(vm))
        mod.storybook.KnobCost = set_type_default() do mod.storybook.KnobCostBase{
            width: Fill
            height: Fill
        }

        let Note = Label{
            text: ""
            draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
        }

        // One version: its knob (or its ring), its name, its numbers and its
        // two buttons.
        let VersionCell = View{
            width: 260.
            height: Fit
            flow: Down
            clip_x: false
            clip_y: false
            spacing: 4.
            align: Align{x: 0.5 y: 0.0}
            slot := mod.storybook.KnobSlot{}
            name := Label{
                text: ""
                draw_text +: {text_style: theme.font_bold{font_size: 11}}
            }
            info := Note{}
            // On the app's own background, so they read on any ground.
            buttons := RoundedView{
                width: Fit
                height: Fit
                flow: Right
                spacing: 6.
                padding: Inset{left: 4. right: 4. top: 3. bottom: 3.}
                show_bg: true
                draw_bg +: {color: theme.color_bg_app border_radius: 6.}
                compile := Button{text: "Compile"}
                measure := Button{text: "Measure"}
            }
        }

        // One version in the sweep's screenshot: its knob and its name.
        // The knob smaller in its square than in the row, so its light and
        // shadow fit in the square.
        let ShotCell = View{
            width: Fit
            height: Fit
            flow: Down
            spacing: 2.
            align: Align{x: 0.5 y: 0.0}
            clip_x: false
            clip_y: false
            slot := mod.storybook.KnobSlot{interactive: false fill: 0.4}
            name := Label{
                text: ""
                draw_text +: {text_style: theme.font_bold{font_size: 10}}
            }
        }

        mod.stories.MaterialKnobCost = mod.storybook.KnobCost{
            flow: Down
            spacing: 14.
            padding: Inset{left: 24. right: 24. top: 20. bottom: 24.}
            scroll_bars: ScrollBars{
                show_scroll_x: false
                show_scroll_y: true
                scroll_bar_y.drag_scrolling: true
            }
            show_bg: true
            draw_bg +: {..mod.storybook.KnobGroundFill}

            // The versions, as knobs; none is built until its Compile.
            versions: {
                preview: mod.storybook.KnobVersionPreview{fill: 0.62}
                bare: mod.storybook.KnobVersionBare{fill: 0.62}
                cuts: mod.storybook.KnobVersionCuts{fill: 0.62}
                wings: mod.storybook.KnobVersionWings{fill: 0.62}
                self_shadow: mod.storybook.KnobVersionSelfShadow{fill: 0.62}
                cast_shadow: mod.storybook.KnobVersionCastShadow{fill: 0.62}
                marks: mod.storybook.KnobVersionMarks{fill: 0.62}
                full: mod.storybook.KnobVersionFull{fill: 0.62}
            }

            intro := P{
                width: Fill
                text: "The knob engine's detail levels, each its own shader. Compile builds a version's knob: the page times it from that moment until the backend has the shader ready, and every launch salts the shader text so no cache can answer (restart for a new cold number). Measure draws a grid of that version's knobs for about two seconds against the same grid empty. Run everything does it all unattended: every compile, then six materials by six styles, each shown and measured, written to a JSON file. The Docs tab says what each version leaves out and what distorts the numbers."
            }
            toolbar := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                spacing: 8.
                align: Align{x: 0.0 y: 0.5}
                actions := RoundedView{
                    width: Fit
                    height: Fit
                    flow: Right
                    spacing: 6.
                    padding: Inset{left: 4. right: 4. top: 3. bottom: 3.}
                    show_bg: true
                    draw_bg +: {color: theme.color_bg_app border_radius: 6.}
                    compile_all := Button{text: "Compile all"}
                    measure_all := Button{text: "Measure all"}
                    run_all := Button{text: "Run everything"}
                    stop := Button{text: "Stop" visible: false}
                    copy_results := Button{text: "Copy results"}
                }
                status := Note{}
            }
            // Where the last "Run everything" wrote its results.
            sweep_note := Note{width: Fill visible: false}
            row := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                spacing: 18.
                clip_x: false
                clip_y: false
                v0 := VersionCell{}
                v1 := VersionCell{}
                v2 := VersionCell{}
                v3 := VersionCell{}
                v4 := VersionCell{}
                v5 := VersionCell{}
                v6 := VersionCell{}
                v7 := VersionCell{}
            }
            // The sweep's screenshot: every version side by side.
            shots := View{
                visible: false
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                spacing: 12.
                clip_x: false
                clip_y: false
                s0 := ShotCell{}
                s1 := ShotCell{}
                s2 := ShotCell{}
                s3 := ShotCell{}
                s4 := ShotCell{}
                s5 := ShotCell{}
                s6 := ShotCell{}
                s7 := ShotCell{}
            }
            grid_box := View{
                visible: false
                width: Fill
                height: Fit
                flow: Down
                spacing: 6.
                clip_x: false
                clip_y: false
                grid_note := Note{}
                grid := mod.storybook.KnobGrid{}
            }
            results := Label{
                width: Fill
                text: ""
                draw_text +: {text_style: theme.font_code{font_size: 9.}}
            }
        }
    }
}

/// How many knobs the measuring grid draws.
const GRID_KNOBS: usize = 24;
/// Before a run measures: the first frames draw what is new (bakes, the
/// pipeline's first use) and are thrown away.
const WARM_SECONDS: f64 = 0.5;
const WARM_FRAMES: usize = 2;
/// A run measures this long, and at least this many frames, and gives up
/// waiting for them after the last.
const RUN_SECONDS: f64 = 2.0;
const RUN_FRAMES: usize = 8;
const RUN_LIMIT_SECONDS: f64 = 20.0;
/// A compile the backend has not finished in this long is reported as such.
const COMPILE_LIMIT_SECONDS: f64 = 120.0;

/// The backend this build draws with.
fn backend_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "D3D11"
    } else if cfg!(any(target_os = "macos", target_os = "ios", target_os = "tvos")) {
        "Metal"
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        "OpenGL"
    } else {
        "WebGL"
    }
}

/// Whether the backend has the shader ready to draw; None where the backend
/// does not say (Metal).
#[cfg(any(target_os = "linux", target_os = "android", target_os = "windows"))]
fn shader_ready(cx: &Cx, shader: DrawShaderId) -> Option<bool> {
    Some(cx.is_draw_shader_window_ready(shader))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "windows")))]
fn shader_ready(_cx: &Cx, _shader: DrawShaderId) -> Option<bool> {
    None
}

/// The generated shader text's size and FNV-1a hash: the hash differs
/// between launches, which is the salt doing its work. Read through the
/// text's `Hash`, which hands over every byte of it.
fn shader_text(cx: &Cx, shader: DrawShaderId) -> (usize, u64) {
    use std::hash::Hash;
    let mut meter = TextMeter { bytes: 0, hash: 0xcbf29ce484222325 };
    cx.draw_shaders[shader.index].mapping.code.hash(&mut meter);
    (meter.bytes, meter.hash)
}

struct TextMeter {
    bytes: usize,
    hash: u64,
}

impl std::hash::Hasher for TextMeter {
    fn write(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len();
        for b in bytes {
            self.hash ^= *b as u64;
            self.hash = self.hash.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) })
}

/// A version's compile, as timed.
#[derive(Clone, Copy)]
struct CompileResult {
    /// From Compile to ready, in ms; None where the backend does not say.
    total_ms: Option<f64>,
    /// The part spent generating the shader text on the UI thread.
    codegen_ms: f64,
    text_bytes: usize,
    text_hash: u64,
    /// The shader was ready the moment the knob was built: this launch had
    /// compiled it already, so the number is not a cold one.
    warm: bool,
    /// The backend never said it was ready.
    timed_out: bool,
    /// The shader did not build (its errors are in the log).
    failed: bool,
}

/// A run of the measuring grid.
#[derive(Clone)]
struct MeasureResult {
    /// Median GPU time of the pass per frame, ms; None when nothing came
    /// back.
    gpu_ms: Option<f64>,
    gpu_samples: usize,
    gpu_dropped: u64,
    /// Median frame interval, ms.
    cpu_ms: Option<f64>,
    frames: usize,
    knobs: usize,
    knob_size: f64,
    /// Style and material, as the run drew them.
    look: String,
    /// The empty grid this run is compared with: its batch's, measured
    /// before the batch and, for more than one version, after it too.
    base_gpu: Option<f64>,
    base_cpu: Option<f64>,
}

/// What survives the page being rebuilt (a theme switch, navigating away
/// and back): the numbers, keyed by version.
#[derive(Default)]
struct KnobCostResults {
    compiled: Vec<Option<CompileResult>>,
    measured: Vec<Option<MeasureResult>>,
    /// The latest batch's empty grid, and each of its runs' CPU and GPU
    /// medians (before, and after).
    baseline: Option<MeasureResult>,
    baseline_runs: Vec<(Option<f64>, Option<f64>)>,
}

impl KnobCostResults {
    fn sized(&mut self) -> &mut Self {
        self.compiled.resize(VERSIONS.len(), None);
        self.measured.resize(VERSIONS.len(), None);
        self
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Slot {
    Baseline,
    Version(usize),
}

/// A compile in flight.
struct Compiling {
    start: f64,
    codegen_ms: f64,
    shader: Option<DrawShaderId>,
    warm: bool,
}

/// A run in flight.
struct Run {
    slot: Slot,
    tag_warm: u64,
    tag_measure: u64,
    start: f64,
    /// Set once the warm-up is over.
    measuring_since: Option<f64>,
    frames_warm: usize,
    last_frame: Option<f64>,
    cpu: Vec<f64>,
    gpu: Vec<f64>,
    dropped_at_start: u64,
    index: usize,
    of: usize,
    /// How long it measures.
    seconds: f64,
}

/// Where the sweep is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SweepPhase {
    /// Compiling every version, one after another.
    Compiling,
    /// The look is set; waiting for it to draw.
    Settle { since: f64, frames: usize },
    /// Holding still for the screenshot.
    Hold { until: f64 },
    /// Measuring every version on the look.
    Measuring,
}

/// The sweep in flight.
struct Sweep {
    combos: Vec<(usize, usize)>,
    step: usize,
    phase: SweepPhase,
    started: f64,
    records: Vec<ComboRecord>,
    /// The look before the sweep, put back after it.
    prior: (KnobMaterial, f64, f64),
}

/// The sweep starts itself once per process (`MAKEPAD_KNOB_COST_SWEEP=1`).
static AUTO_SWEEP_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Before it does, the page is open this long, so a page opened only on the
/// way to another (a remembered last story) starts nothing.
const AUTO_SWEEP_DELAY: f64 = 1.5;

#[derive(Script, Widget)]
pub struct KnobCost {
    #[deref]
    view: View,
    #[live]
    look: KnobLook,
    /// The versions' knobs as templates, by `KnobVersion::key`.
    #[live]
    versions: ScriptObjectRef,
    /// The row's knob size and the measuring grid's, in points.
    #[live(200.0)]
    row_size: f64,
    #[live(96.0)]
    grid_size: f64,
    /// The knobs' size in the sweep's screenshot.
    #[live(200.0)]
    shot_size: f64,
    /// Each version's knob, once built.
    #[rust]
    knobs: Vec<Option<WidgetRef>>,
    #[rust]
    compiling: Vec<Option<Compiling>>,
    #[rust]
    compile_queue: VecDeque<usize>,
    /// The runs to make, and the ones waiting for their compiles first.
    #[rust]
    measure_queue: VecDeque<Slot>,
    #[rust]
    measure_waiting: Vec<Slot>,
    #[rust]
    run: Option<Run>,
    /// This batch's empty-grid runs and the versions it measured.
    #[rust]
    batch_baselines: Vec<MeasureResult>,
    #[rust]
    batch_versions: Vec<usize>,
    #[rust]
    runs_started: u64,
    #[rust]
    runs_total: usize,
    #[rust]
    next_frame: NextFrame,
    /// The pass the page draws into, whose GPU time a run reads.
    #[rust]
    pass: Option<DrawPassId>,
    #[rust]
    gpu_buf: Vec<(u64, f64)>,
    /// What the knobs were last handed.
    #[rust]
    pushed: Option<(KnobMaterial, usize, f64, bool)>,
    #[rust]
    ground_luma: Option<f64>,
    /// The cells' width, set from the row's knob size: fixed, so a cell does
    /// not move as its numbers change.
    #[rust]
    cell_width: f64,
    #[rust]
    restored: bool,
    #[rust]
    sweep: Option<Sweep>,
    /// When the sweep starts itself, if the environment asks it to.
    #[rust]
    sweep_armed: Option<f64>,
    /// Where the last sweep wrote its results.
    #[rust]
    sweep_path: Option<String>,
    /// The window's DPI factor, as the page last drew.
    #[rust]
    dpi: f64,
}

impl ScriptHook for KnobCost {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.look.reset();
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        vm.with_cx_mut(|cx| self.view.redraw(cx));
    }
}

impl KnobCost {
    fn cell(i: usize) -> LiveId {
        LiveId::from_str(&format!("v{i}"))
    }

    fn shot(i: usize) -> LiveId {
        LiveId::from_str(&format!("s{i}"))
    }

    fn sized(&mut self) {
        if self.knobs.len() != VERSIONS.len() {
            self.knobs.resize_with(VERSIONS.len(), || None);
            self.compiling.resize_with(VERSIONS.len(), || None);
        }
    }

    fn busy(&self) -> bool {
        self.run.is_some()
            || self.compiling.iter().any(|c| c.is_some())
            || self.sweep.is_some()
            || self.sweep_armed.is_some()
    }

    fn template(&self, cx: &mut Cx, i: usize) -> Option<ScriptValue> {
        let key = LiveId::from_str(VERSIONS[i].key);
        let obj = self.versions.as_object();
        let value = cx.with_vm(|vm| vm.bx.heap.value(obj, key.into(), NoTrap));
        value.as_object().map(|o| o.into())
    }

    /// Build version `i`'s knob now: this is where its shader's text is
    /// generated and queued, and where its timing starts.
    fn start_compile(&mut self, cx: &mut Cx, i: usize) {
        self.sized();
        if self.knobs[i].is_some() || self.compiling[i].is_some() {
            return;
        }
        let Some(template) = self.template(cx, i) else {
            log!("knob cost: no template for {}", VERSIONS[i].key);
            return;
        };
        let start = Cx::monotonic_now();
        let knob = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template));
        let codegen_ms = (Cx::monotonic_now() - start) * 1000.0;
        let shader = knob.borrow::<TurnedKnob>().and_then(|k| k.draw_shader_id());
        // Ready before any frame has passed: this launch compiled the same
        // shader already, and the number would not be a cold one.
        let warm = shader.and_then(|s| shader_ready(cx, s)).unwrap_or(false);
        self.knobs[i] = Some(knob);
        self.compiling[i] = Some(Compiling { start, codegen_ms, shader, warm });
        self.pushed = None;
        self.next_frame = cx.new_next_frame();
        self.view.redraw(cx);
    }

    /// Compile these, one after another so no two share the machine.
    fn queue_compiles(&mut self, cx: &mut Cx, list: &[usize]) {
        self.sized();
        for &i in list {
            if self.knobs[i].is_none() && self.compiling[i].is_none() && !self.compile_queue.contains(&i) {
                self.compile_queue.push_back(i);
            }
        }
        if !self.compiling.iter().any(|c| c.is_some()) {
            if let Some(next) = self.compile_queue.pop_front() {
                self.start_compile(cx, next);
            }
        }
        self.refresh_status(cx);
    }

    /// Measure these, each against the empty grid; versions not compiled
    /// yet are compiled first.
    fn queue_measures(&mut self, cx: &mut Cx, versions: &[usize]) {
        self.sized();
        if self.run.is_some() || !self.measure_queue.is_empty() || !self.measure_waiting.is_empty() {
            return;
        }
        let missing: Vec<usize> =
            versions.iter().copied().filter(|&i| self.knobs[i].is_none() || self.compiling[i].is_some()).collect();
        // The empty grid before the versions and, for more than one, after
        // them as well: the two together say how much the machine drifted.
        let mut slots = vec![Slot::Baseline];
        slots.extend(versions.iter().map(|&i| Slot::Version(i)));
        if versions.len() > 1 {
            slots.push(Slot::Baseline);
        }
        self.measure_waiting = slots;
        self.queue_compiles(cx, &missing);
        // Nothing to wait for (or nothing that could be built): start now.
        if !self.compiling.iter().any(|c| c.is_some()) {
            self.start_waiting_measures(cx);
        }
    }

    /// The runs that waited for their compiles, less a version that failed
    /// to build.
    fn start_waiting_measures(&mut self, cx: &mut Cx) {
        let slots: Vec<Slot> = std::mem::take(&mut self.measure_waiting)
            .into_iter()
            .filter(|s| match s {
                Slot::Baseline => true,
                Slot::Version(i) => self.knobs[*i].is_some(),
            })
            .collect();
        if !slots.is_empty() {
            self.start_measuring(cx, slots);
        }
    }

    fn start_measuring(&mut self, cx: &mut Cx, slots: Vec<Slot>) {
        self.batch_baselines.clear();
        self.batch_versions.clear();
        self.runs_total = slots.len();
        self.measure_queue = slots.into();
        if let Some(pass) = self.pass {
            cx.passes[pass].set_gpu_timing_enabled(true);
            // What an earlier opt-in left behind is not this run's.
            self.gpu_buf.clear();
            cx.passes[pass].drain_gpu_time_samples(&mut self.gpu_buf);
            self.gpu_buf.clear();
        }
        self.view.set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.next_run(cx);
    }

    fn next_run(&mut self, cx: &mut Cx) {
        let Some(slot) = self.measure_queue.pop_front() else {
            self.finish_measuring(cx);
            return;
        };
        self.runs_started += 1;
        let base = 0x4b4e_0000_0000u64 | (self.runs_started << 4);
        let dropped_at_start = self.pass.map(|p| cx.passes[p].gpu_time_samples_dropped()).unwrap_or(0);
        let index = self.runs_total - self.measure_queue.len();
        self.run = Some(Run {
            slot,
            tag_warm: base | 1,
            tag_measure: base | 2,
            start: Cx::monotonic_now(),
            measuring_since: None,
            frames_warm: 0,
            last_frame: None,
            cpu: Vec::new(),
            gpu: Vec::new(),
            dropped_at_start,
            index,
            of: self.runs_total,
            seconds: if self.sweep.is_some() { sweep::RUN_SECONDS } else { RUN_SECONDS },
        });
        let knob = match slot {
            Slot::Baseline => None,
            Slot::Version(i) => self.knobs[i].clone(),
        };
        if let Some(mut grid) = self.view.widget(cx, ids!(grid)).borrow_mut::<KnobGrid>() {
            grid.knob = knob;
            grid.count = GRID_KNOBS;
            grid.size = self.grid_size;
        }
        self.view.view(cx, ids!(row)).set_visible(cx, false);
        self.view.view(cx, ids!(grid_box)).set_visible(cx, true);
        let what = match slot {
            Slot::Baseline => "the empty grid (the baseline)".to_string(),
            Slot::Version(i) => format!("{} knobs of {}", GRID_KNOBS, VERSIONS[i].name),
        };
        let note = format!("Measuring {what}. The page redraws every frame until the run is over.");
        self.view.label(cx, ids!(grid_note)).set_text(cx, &note);
        self.next_frame = cx.new_next_frame();
        self.view.redraw(cx);
    }

    fn finish_run(&mut self, cx: &mut Cx) {
        let Some(run) = self.run.take() else {
            return;
        };
        let dropped = self.pass.map(|p| cx.passes[p].gpu_time_samples_dropped()).unwrap_or(0);
        let result = MeasureResult {
            gpu_ms: median(&run.gpu),
            gpu_samples: run.gpu.len(),
            gpu_dropped: dropped.saturating_sub(run.dropped_at_start),
            cpu_ms: median(&run.cpu),
            frames: run.cpu.len(),
            knobs: GRID_KNOBS,
            knob_size: self.grid_size,
            look: format!("{} in {}", STYLES[self.look.style_index()].label(), self.look.material_name()),
            base_gpu: None,
            base_cpu: None,
        };
        match run.slot {
            Slot::Baseline => self.batch_baselines.push(result),
            Slot::Version(i) => {
                self.batch_versions.push(i);
                cx.global::<KnobCostResults>().sized().measured[i] = Some(result);
            }
        }
    }

    /// The batch's empty grid: the mean of its runs' medians, handed to
    /// every version the batch measured.
    fn settle_baseline(&mut self, cx: &mut Cx) {
        let runs = std::mem::take(&mut self.batch_baselines);
        let Some(first) = runs.first().cloned() else {
            return;
        };
        let mean = |f: &dyn Fn(&MeasureResult) -> Option<f64>| {
            let v: Vec<f64> = runs.iter().filter_map(f).collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        };
        let base_gpu = mean(&|m| m.gpu_ms);
        let base_cpu = mean(&|m| m.cpu_ms);
        let combined = MeasureResult {
            gpu_ms: base_gpu,
            gpu_samples: runs.iter().map(|m| m.gpu_samples).sum(),
            gpu_dropped: runs.iter().map(|m| m.gpu_dropped).sum(),
            cpu_ms: base_cpu,
            frames: runs.iter().map(|m| m.frames).sum(),
            base_gpu,
            base_cpu,
            ..first
        };
        let results = cx.global::<KnobCostResults>().sized();
        results.baseline_runs = runs.iter().map(|m| (m.gpu_ms, m.cpu_ms)).collect();
        results.baseline = Some(combined);
        for &i in &self.batch_versions {
            if let Some(m) = &mut results.measured[i] {
                m.base_gpu = base_gpu;
                m.base_cpu = base_cpu;
            }
        }
        self.batch_versions.clear();
    }

    fn finish_measuring(&mut self, cx: &mut Cx) {
        if let Some(pass) = self.pass {
            cx.passes[pass].set_gpu_timing_enabled(false);
        }
        if let Some(mut grid) = self.view.widget(cx, ids!(grid)).borrow_mut::<KnobGrid>() {
            grid.knob = None;
        }
        // In a sweep the next look puts up its own layout.
        self.view.view(cx, ids!(row)).set_visible(cx, self.sweep.is_none());
        self.view.view(cx, ids!(grid_box)).set_visible(cx, false);
        self.runs_total = 0;
        self.settle_baseline(cx);
        if self.sweep.is_none() {
            let table = self.table(cx);
            log!("knob cost: measured\n{table}");
        }
        self.refresh_texts(cx);
        self.view.redraw(cx);
    }

    /// Once a frame while anything is in flight: poll the compiles, step the
    /// run, read the GPU samples.
    fn on_frame(&mut self, cx: &mut Cx) {
        self.sized();
        let now = Cx::monotonic_now();
        // THE COMPILES.
        for i in 0..VERSIONS.len() {
            let Some(c) = &self.compiling[i] else {
                continue;
            };
            let elapsed = now - c.start;
            let ready = match c.shader {
                Some(shader) => shader_ready(cx, shader),
                None => None,
            };
            let timed_out = elapsed > COMPILE_LIMIT_SECONDS;
            if ready == Some(false) && !timed_out {
                continue;
            }
            let (text_bytes, text_hash) = c.shader.map(|s| shader_text(cx, s)).unwrap_or((0, 0));
            let result = CompileResult {
                total_ms: ready.map(|_| elapsed * 1000.0),
                codegen_ms: c.codegen_ms,
                text_bytes,
                text_hash,
                warm: c.warm,
                timed_out,
                failed: c.shader.is_none(),
            };
            if result.failed {
                log!("knob cost: {} built no shader (see the shader log above)", VERSIONS[i].name);
                self.knobs[i] = None;
            }
            log!(
                "knob cost: {} {} in {} (codegen {:.1} ms), shader text {} bytes, fnv {:016x}, salt {}",
                VERSIONS[i].name,
                if c.warm { "was compiled already" } else { "cold compile" },
                result.total_ms.map(|ms| format!("{ms:.1} ms")).unwrap_or_else(|| "not measured".into()),
                c.codegen_ms,
                text_bytes,
                text_hash,
                launch_salt()
            );
            cx.global::<KnobCostResults>().sized().compiled[i] = Some(result);
            self.compiling[i] = None;
            self.pushed = None;
            if let Some(next) = self.compile_queue.pop_front() {
                self.start_compile(cx, next);
            }
        }
        let compiles_done = !self.compiling.iter().any(|c| c.is_some()) && self.compile_queue.is_empty();
        if compiles_done && !self.measure_waiting.is_empty() {
            self.start_waiting_measures(cx);
        }
        // THE RUN.
        if let Some(pass) = self.pass {
            self.gpu_buf.clear();
            cx.passes[pass].drain_gpu_time_samples(&mut self.gpu_buf);
        }
        let mut run_over = false;
        if let Some(run) = &mut self.run {
            for &(tag, ms) in &self.gpu_buf {
                if tag == run.tag_measure {
                    run.gpu.push(ms);
                }
            }
            match run.measuring_since {
                None => {
                    run.frames_warm += 1;
                    if now - run.start >= WARM_SECONDS && run.frames_warm >= WARM_FRAMES {
                        run.measuring_since = Some(now);
                        run.last_frame = Some(now);
                    }
                }
                Some(since) => {
                    if let Some(last) = run.last_frame {
                        run.cpu.push((now - last) * 1000.0);
                    }
                    run.last_frame = Some(now);
                    let long_enough = now - since >= run.seconds && run.cpu.len() >= RUN_FRAMES;
                    if long_enough || now - run.start >= RUN_LIMIT_SECONDS {
                        run_over = true;
                    }
                }
            }
        }
        if run_over {
            self.finish_run(cx);
            self.next_run(cx);
        }
        // THE SWEEP.
        self.step_sweep(cx, now);
        self.refresh_status(cx);
        self.refresh_texts(cx);
        if self.busy() {
            self.next_frame = cx.new_next_frame();
            // Holding for a screenshot, or only waiting to start, the page
            // keeps still.
            let holding = matches!(self.sweep.as_ref().map(|s| s.phase), Some(SweepPhase::Hold { .. }));
            let waiting = self.sweep.is_none() && self.sweep_armed.is_some() && self.run.is_none();
            if !holding && !waiting {
                self.view.redraw(cx);
            }
        }
    }

    // ---- the sweep ("Run everything") ----

    /// Start the sweep: every version compiled cold (those already built
    /// this launch keep their numbers), then each look in turn.
    fn start_sweep(&mut self, cx: &mut Cx) {
        self.sized();
        let measuring = self.run.is_some() || !self.measure_queue.is_empty() || !self.measure_waiting.is_empty();
        if self.sweep.is_some() || measuring {
            return;
        }
        let combos = sweep::combinations();
        log!("KNOBCOST START steps={} versions={} backend={}", combos.len(), VERSIONS.len(), backend_name());
        self.sweep = Some(Sweep {
            combos,
            step: 0,
            phase: SweepPhase::Compiling,
            started: Cx::time_now(),
            records: Vec::new(),
            prior: (self.look.material(), self.look.style, self.look.value),
        });
        self.sweep_path = None;
        self.view.button(cx, ids!(stop)).set_visible(cx, true);
        self.view.set_scroll_pos(cx, dvec2(0.0, 0.0));
        let all: Vec<usize> = (0..VERSIONS.len()).collect();
        self.queue_compiles(cx, &all);
        self.next_frame = cx.new_next_frame();
        self.view.redraw(cx);
    }

    /// Show the page as the sweep's screenshot wants it (every version side
    /// by side), or as it normally is.
    fn sweep_layout(&mut self, cx: &mut Cx, shots: bool) {
        self.view.widget(cx, ids!(intro)).set_visible(cx, !shots);
        self.view.widget(cx, ids!(results)).set_visible(cx, !shots);
        self.view.view(cx, ids!(row)).set_visible(cx, !shots);
        self.view.view(cx, ids!(shots)).set_visible(cx, shots);
    }

    /// Put up look `step`: its material and style, every version beside the
    /// others, waiting for it to draw.
    fn enter_combo(&mut self, cx: &mut Cx, now: f64) {
        let Some(sw) = &mut self.sweep else {
            return;
        };
        let (m, style) = sw.combos[sw.step];
        sw.phase = SweepPhase::Settle { since: now, frames: 0 };
        self.look.load(&MATERIALS[m]);
        self.look.style = style as f64;
        self.look.lit = false;
        self.pushed = None;
        self.sweep_layout(cx, true);
        self.view.view(cx, ids!(grid_box)).set_visible(cx, false);
        self.view.set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.view.redraw(cx);
    }

    /// Where each version's knob stands in the screenshot, window-local.
    fn shot_rects(&mut self, cx: &mut Cx) -> Vec<Option<[f64; 4]>> {
        (0..VERSIONS.len())
            .map(|i| {
                let slot = self.view.widget(cx, &[Self::shot(i), live_id!(slot)]);
                let knob = slot.borrow::<KnobSlot>().and_then(|s| s.knob.clone())?;
                let area = knob.area();
                if !area.is_valid(cx) {
                    return None;
                }
                let r = area.clipped_rect(cx);
                (r.size.x > 0.0 && r.size.y > 0.0).then_some([r.pos.x, r.pos.y, r.size.x, r.size.y])
            })
            .collect()
    }

    fn step_sweep(&mut self, cx: &mut Cx, now: f64) {
        if let Some(at) = self.sweep_armed {
            if now >= at {
                self.sweep_armed = None;
                if !AUTO_SWEEP_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    self.start_sweep(cx);
                }
            }
        }
        let Some(phase) = self.sweep.as_ref().map(|s| s.phase) else {
            return;
        };
        match phase {
            SweepPhase::Compiling => {
                if !self.compiling.iter().any(|c| c.is_some()) && self.compile_queue.is_empty() {
                    self.enter_combo(cx, now);
                }
            }
            SweepPhase::Settle { since, frames } => {
                let frames = frames + 1;
                // A few frames and half a second: the bakes, the pipelines'
                // first use and the layout have all happened.
                if frames >= 4 && now - since >= 0.5 {
                    let rects = self.shot_rects(cx);
                    let Some(sw) = &mut self.sweep else {
                        return;
                    };
                    let (m, style) = sw.combos[sw.step];
                    let list: Vec<String> = rects
                        .iter()
                        .enumerate()
                        .map(|(i, r)| match r {
                            Some(r) => format!("{}:{:.1},{:.1},{:.1},{:.1}", VERSIONS[i].key, r[0], r[1], r[2], r[3]),
                            None => format!("{}:none", VERSIONS[i].key),
                        })
                        .collect();
                    log!(
                        "KNOBCOST READY step={}/{} material=\"{}\" style=\"{}\" hold={} rects={}",
                        sw.step + 1,
                        sw.combos.len(),
                        sweep::material_name(m),
                        sweep::style_name(style),
                        sweep::HOLD_SECONDS,
                        list.join(";")
                    );
                    sw.records.push(ComboRecord {
                        step: sw.step + 1,
                        material: m,
                        style,
                        rects,
                        ..Default::default()
                    });
                    sw.phase = SweepPhase::Hold { until: now + sweep::HOLD_SECONDS };
                } else if let Some(sw) = &mut self.sweep {
                    sw.phase = SweepPhase::Settle { since, frames };
                }
            }
            SweepPhase::Hold { until } => {
                if now >= until {
                    if let Some(sw) = &mut self.sweep {
                        sw.phase = SweepPhase::Measuring;
                    }
                    self.sweep_layout(cx, false);
                    self.view.view(cx, ids!(row)).set_visible(cx, false);
                    let results = cx.global::<KnobCostResults>().sized();
                    for m in results.measured.iter_mut() {
                        *m = None;
                    }
                    let built: Vec<usize> = (0..VERSIONS.len()).filter(|&i| self.knobs[i].is_some()).collect();
                    self.queue_measures(cx, &built);
                }
            }
            SweepPhase::Measuring => {
                if self.run.is_none() && self.measure_queue.is_empty() && self.measure_waiting.is_empty() {
                    self.record_combo(cx);
                    let more = match &mut self.sweep {
                        Some(sw) => {
                            sw.step += 1;
                            sw.step < sw.combos.len()
                        }
                        None => false,
                    };
                    if more {
                        self.enter_combo(cx, now);
                    } else {
                        self.finish_sweep(cx, false);
                    }
                }
            }
        }
    }

    /// The look's numbers, from the batch that just finished.
    fn record_combo(&mut self, cx: &mut Cx) {
        let (baseline, baseline_runs, measured) = {
            let r = cx.global::<KnobCostResults>().sized();
            (r.baseline.clone(), r.baseline_runs.clone(), r.measured.clone())
        };
        let run = |m: &MeasureResult| RunRecord {
            gpu_ms: m.gpu_ms,
            gpu_samples: m.gpu_samples,
            cpu_ms: m.cpu_ms,
            frames: m.frames,
        };
        let Some(record) = self.sweep.as_mut().and_then(|sw| sw.records.last_mut()) else {
            return;
        };
        record.baseline = baseline.as_ref().map(run).unwrap_or_default();
        record.baseline_runs = baseline_runs;
        record.versions = measured.iter().map(|m| m.as_ref().map(run)).collect();
    }

    /// Stop where the sweep is: what is measured so far is written.
    fn stop_sweep(&mut self, cx: &mut Cx) {
        if self.sweep.is_none() {
            return;
        }
        self.run = None;
        self.measure_queue.clear();
        self.measure_waiting.clear();
        self.batch_baselines.clear();
        self.batch_versions.clear();
        self.compile_queue.clear();
        self.runs_total = 0;
        if let Some(pass) = self.pass {
            cx.passes[pass].set_gpu_timing_enabled(false);
        }
        if let Some(mut grid) = self.view.widget(cx, ids!(grid)).borrow_mut::<KnobGrid>() {
            grid.knob = None;
        }
        self.view.view(cx, ids!(grid_box)).set_visible(cx, false);
        self.finish_sweep(cx, true);
    }

    /// Write the results (file, clipboard, log), put the page and its look
    /// back.
    fn finish_sweep(&mut self, cx: &mut Cx, stopped: bool) {
        let Some(sw) = self.sweep.take() else {
            return;
        };
        let compiled = cx.global::<KnobCostResults>().sized().compiled.clone();
        let compiles: Vec<CompileRecord> = compiled
            .iter()
            .map(|c| match c {
                None => CompileRecord { state: "none", ..Default::default() },
                Some(c) => CompileRecord {
                    state: if c.failed {
                        "failed"
                    } else if c.timed_out {
                        "timed_out"
                    } else if c.warm {
                        "warm"
                    } else if c.total_ms.is_none() {
                        "not_measured"
                    } else {
                        "cold"
                    },
                    total_ms: c.total_ms,
                    codegen_ms: Some(c.codegen_ms),
                    text_bytes: Some(c.text_bytes),
                    text_fnv: Some(c.text_hash),
                },
            })
            .collect();
        let adapter = {
            let r = cx.gpu_info().renderer.clone();
            (!r.is_empty() && r != "unknown").then_some(r)
        };
        let finished = Cx::time_now();
        let shot = self.view.widget(cx, &[Self::shot(0), live_id!(slot)]);
        let shot_fill = shot.borrow::<KnobSlot>().map(|s| s.fill).unwrap_or(0.0);
        let grid_fill = self.view.widget(cx, ids!(grid)).borrow::<KnobGrid>().map(|g| g.fill).unwrap_or(0.0);
        let report = SweepReport {
            backend: backend_name(),
            adapter: adapter.as_deref(),
            started: sw.started,
            finished,
            stopped,
            grid_knobs: GRID_KNOBS,
            grid_size: self.grid_size,
            shot_size: self.shot_size,
            dpi_factor: self.dpi,
            shot_fill,
            grid_fill,
            warm_seconds: WARM_SECONDS,
            compiles: &compiles,
            combos: &sw.records,
            steps: sw.combos.len(),
        };
        let json = report.to_json();
        cx.copy_to_clipboard(&json);
        let word = if stopped { "STOPPED" } else { "DONE" };
        match sweep::write_results(&json, finished) {
            Ok(path) => {
                log!("KNOBCOST {word} path={path} steps={}/{}", sw.records.len(), sw.combos.len());
                self.sweep_path = Some(path);
            }
            Err(e) => {
                log!("KNOBCOST {word} path=none error=\"{e}\" (the JSON is on the clipboard)");
                self.sweep_path = Some(format!("not written ({e}); the JSON is on the clipboard"));
            }
        }
        // The look the page had before.
        let (m, style, value) = sw.prior;
        self.look.load(&m);
        self.look.style = style;
        self.look.value = value;
        self.pushed = None;
        self.sweep_layout(cx, false);
        self.view.button(cx, ids!(stop)).set_visible(cx, false);
        self.refresh_texts(cx);
        self.refresh_status(cx);
        self.view.redraw(cx);
    }

    /// The toolbar's line: what is in flight.
    fn refresh_status(&mut self, cx: &mut Cx) {
        let now = Cx::monotonic_now();
        let mut line = String::new();
        if let Some(sw) = &self.sweep {
            let look = sw.combos.get(sw.step).map(|&(m, style)| {
                format!("{} in {}", sweep::style_name(style), sweep::material_name(m))
            });
            let step = format!("step {}/{}", (sw.step + 1).min(sw.combos.len()), sw.combos.len());
            line = match sw.phase {
                SweepPhase::Compiling => {
                    let results = cx.global::<KnobCostResults>().sized();
                    let done = results.compiled.iter().filter(|c| c.is_some()).count();
                    match self.compiling.iter().position(|c| c.is_some()) {
                        Some(i) => {
                            let (name, of) = (VERSIONS[i].name, VERSIONS.len());
                            format!("Run everything: compiling {name} ({} of {of})", done + 1)
                        }
                        None => "Run everything: compiling".to_string(),
                    }
                }
                SweepPhase::Settle { .. } | SweepPhase::Hold { .. } => {
                    format!("Run everything: {step}, {}: holding for the screenshot", look.unwrap_or_default())
                }
                SweepPhase::Measuring => {
                    let what = match self.run.as_ref().map(|r| r.slot) {
                        Some(Slot::Baseline) => "the empty grid".to_string(),
                        Some(Slot::Version(i)) => VERSIONS[i].name.to_string(),
                        None => "...".to_string(),
                    };
                    format!("Run everything: {step}, {}, measuring {what}", look.unwrap_or_default())
                }
            };
        } else if let Some(run) = &self.run {
            let what = match run.slot {
                Slot::Baseline => "the empty grid".to_string(),
                Slot::Version(i) => VERSIONS[i].name.to_string(),
            };
            let phase = if run.measuring_since.is_some() { "measuring" } else { "warming up" };
            line = format!("Run {} of {}: {what}, {phase}, {:.1} s", run.index, run.of, now - run.start);
        } else if let Some((i, c)) = self.compiling.iter().enumerate().find_map(|(i, c)| c.as_ref().map(|c| (i, c))) {
            let queued = self.compile_queue.len();
            line = format!("Compiling {}: {:.1} s", VERSIONS[i].name, now - c.start);
            if queued > 0 {
                line.push_str(&format!(", {queued} more queued"));
            }
            if !self.measure_waiting.is_empty() {
                line.push_str(", then measuring");
            }
        }
        let backend = backend_name();
        if line.is_empty() {
            line = format!("{backend}. Idle: nothing redraws until you press a button.");
        }
        self.view.label(cx, ids!(status)).set_text(cx, &line);
        let note = match (&self.sweep_path, &self.sweep) {
            (Some(path), None) => format!("Run everything wrote {path} (the JSON is on the clipboard too)."),
            _ => String::new(),
        };
        let label = self.view.label(cx, ids!(sweep_note));
        label.set_text(cx, &note);
        label.set_visible(cx, !note.is_empty());
    }

    /// Every cell's text and the table.
    fn refresh_texts(&mut self, cx: &mut Cx) {
        self.sized();
        let now = Cx::monotonic_now();
        let (compiled, measured) = {
            let r = cx.global::<KnobCostResults>().sized();
            (r.compiled.clone(), r.measured.clone())
        };
        for i in 0..VERSIONS.len() {
            let mut lines = Vec::new();
            if let Some(c) = &self.compiling[i] {
                lines.push(format!("compiling... {:.1} s", now - c.start));
            } else if let Some(c) = &compiled[i] {
                lines.push(compile_line(c));
            } else {
                lines.push("not compiled".to_string());
            }
            if let Some(m) = &measured[i] {
                let gpu = per_knob(m.gpu_ms, m.base_gpu, m.knobs, "time: not recorded on this backend");
                lines.push(format!("GPU {gpu}"));
                lines.push(format!("CPU {}", per_knob(m.cpu_ms, m.base_cpu, m.knobs, "frame: none measured")));
            }
            self.view.label(cx, &[Self::cell(i), live_id!(name)]).set_text(cx, VERSIONS[i].name);
            self.view.label(cx, &[Self::cell(i), live_id!(info)]).set_text(cx, &lines.join("\n"));
        }
        let table = self.table(cx);
        self.view.label(cx, ids!(results)).set_text(cx, &table);
    }

    /// The results as a plain-text table, for the page, the log and the
    /// clipboard.
    fn table(&mut self, cx: &mut Cx) -> String {
        let (compiled, measured, baseline, baseline_runs) = {
            let r = cx.global::<KnobCostResults>().sized();
            (r.compiled.clone(), r.measured.clone(), r.baseline.clone(), r.baseline_runs.clone())
        };
        let adapter = {
            let r = &cx.gpu_info().renderer;
            if r.is_empty() || r == "unknown" {
                "not exposed by the platform".to_string()
            } else {
                r.clone()
            }
        };
        let base_gpu = baseline.as_ref().and_then(|b| b.gpu_ms);
        let base_cpu = baseline.as_ref().and_then(|b| b.cpu_ms);
        let mut out = String::new();
        out.push_str(&format!(
            "Knob cost | backend {} | adapter {} | salt {}\n",
            backend_name(),
            adapter,
            launch_salt()
        ));
        if let Some(b) = &baseline {
            let runs: Vec<String> = baseline_runs
                .iter()
                .map(|(g, c)| match g {
                    Some(g) => format!("GPU {g:.2} / CPU {}", c.map(|c| format!("{c:.2}")).unwrap_or("-".into())),
                    None => format!("CPU {}", c.map(|c| format!("{c:.2}")).unwrap_or("-".into())),
                })
                .collect();
            out.push_str(&format!(
                "grid {} knobs at {:.0} pt | {} | empty grid runs (ms): {}\n",
                b.knobs,
                b.knob_size,
                b.look,
                runs.join(", then ")
            ));
        }
        // Two tables, so each fits the page: the compiles, then the frame
        // cost.
        let row1 = |cells: [&str; 5]| {
            format!("{:<21}{:>11}{:>10}{:>8}{:>11}\n", cells[0], cells[1], cells[2], cells[3], cells[4])
        };
        let row2 = |cells: [&str; 8]| {
            format!(
                "{:<21}{:>10}{:>10}{:>10}{:>10}{:>10}{:>10}{:>7}\n",
                cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7]
            )
        };
        out.push_str(&row1(["version", "compile", "codegen", "text", "text fnv"]));
        let mut costs =
            row2(["version", "GPU frm", "GPU +grd", "GPU/knob", "CPU frm", "CPU +grd", "CPU/knob", "frames"]);
        let ms = |v: Option<f64>, none: &str| v.map(|v| format!("{v:.2} ms")).unwrap_or_else(|| none.to_string());
        let gpu_none = if baseline.as_ref().is_some_and(|b| b.gpu_samples == 0) { "n/r" } else { "-" };
        let frames = |m: Option<&MeasureResult>| m.map(|m| m.frames.to_string()).unwrap_or_else(|| "-".into());
        costs.push_str(&row2([
            "empty grid",
            &ms(base_gpu, gpu_none),
            "-",
            "-",
            &ms(base_cpu, "-"),
            "-",
            "-",
            &frames(baseline.as_ref()),
        ]));
        let mut dropped = baseline.as_ref().map(|b| b.gpu_dropped).unwrap_or(0);
        for i in 0..VERSIONS.len() {
            let c = compiled[i];
            let m = measured[i].as_ref();
            dropped += m.map(|m| m.gpu_dropped).unwrap_or(0);
            let compile = match c {
                None => "-".to_string(),
                Some(c) if c.failed => "failed".to_string(),
                Some(c) if c.timed_out => "timed out".to_string(),
                Some(c) if c.warm => "warm".to_string(),
                Some(c) => c.total_ms.map(|v| format!("{v:.1} ms")).unwrap_or_else(|| "n/m".to_string()),
            };
            let codegen = c.map(|c| format!("{:.1} ms", c.codegen_ms)).unwrap_or_else(|| "-".into());
            let text = c.map(|c| format!("{} KB", (c.text_bytes + 512) / 1024)).unwrap_or_else(|| "-".into());
            let hash = c.map(|c| format!("{:08x}", c.text_hash >> 32)).unwrap_or_else(|| "-".into());
            out.push_str(&row1([VERSIONS[i].name, &compile, &codegen, &text, &hash]));
            let gpu_none = if m.is_some_and(|m| m.gpu_samples == 0) { "n/r" } else { "-" };
            let (gpu_frame, gpu_grid, gpu_knob) =
                columns(m.and_then(|m| m.gpu_ms), m.and_then(|m| m.base_gpu), m.map(|m| m.knobs), gpu_none);
            let (cpu_frame, cpu_grid, cpu_knob) =
                columns(m.and_then(|m| m.cpu_ms), m.and_then(|m| m.base_cpu), m.map(|m| m.knobs), "-");
            costs.push_str(&row2([
                VERSIONS[i].name,
                &gpu_frame,
                &gpu_grid,
                &gpu_knob,
                &cpu_frame,
                &cpu_grid,
                &cpu_knob,
                &frames(m),
            ]));
        }
        out.push('\n');
        out.push_str(&costs);
        out.push('\n');
        out.push_str(&format!(
            "compile: cold, from Compile until the backend has the shader ready, polled once a frame{}; \
             warm = compiled earlier this launch. codegen: generating the shader text on the UI thread, part of \
             compile. text fnv: differs every launch (the salt).\n",
            match backend_name() {
                "Metal" => " (not measured on Metal: n/m)",
                _ => "",
            }
        ));
        out.push_str(
            "GPU frm: the pass's GPU time per frame (median); n/r = not recorded on this backend. CPU frm: the \
             frame interval (median), vsync-bound where the display syncs.\n",
        );
        out.push_str(
            "+grd: less the empty grid measured in the same batch; per knob: that over the grid's knobs. frames: \
             frames measured.\n",
        );
        if dropped > 0 {
            out.push_str(&format!("GPU samples the platform dropped (queue full or no query free): {dropped}\n"));
        }
        out.pop();
        out
    }

    /// Hand the look to every knob built, and colour the text for the
    /// ground.
    fn push(&mut self, cx: &mut Cx) {
        self.sized();
        let m = self.look.material();
        let style = self.look.style_index();
        let state = (m, style, self.look.value, self.look.lit);
        if self.pushed != Some(state) {
            self.pushed = Some(state);
            for knob in self.knobs.iter().flatten() {
                if let Some(mut k) = knob.borrow_mut::<TurnedKnob>() {
                    k.set_style(cx, style);
                    k.set_material(cx, &m);
                    k.set_value(cx, self.look.value);
                    k.set_lit(cx, self.look.lit);
                }
            }
        }
        let ground = crate::knob::bake::ink(m.ground);
        let luma = ground[0] * 0.2126 + ground[1] * 0.7152 + ground[2] * 0.0722;
        if self.ground_luma != Some(luma) {
            self.ground_luma = Some(luma);
            let text: Vec4f = if luma > 0.45 { vec4(0.14, 0.15, 0.18, 1.0) } else { vec4(0.86, 0.88, 0.91, 1.0) };
            let meta: Vec4f = if luma > 0.45 { vec4(0.30, 0.32, 0.37, 1.0) } else { vec4(0.62, 0.65, 0.70, 1.0) };
            let ring: Vec4f = if luma > 0.45 { vec4(0.2, 0.22, 0.26, 0.55) } else { vec4(0.8, 0.82, 0.86, 0.55) };
            for i in 0..VERSIONS.len() {
                let mut name = self.view.widget(cx, &[Self::shot(i), live_id!(name)]);
                script_apply_eval!(cx, name, { draw_text +: {color: #(text)} });
            }
            let notes = [live_id!(intro), live_id!(status), live_id!(sweep_note), live_id!(grid_note)];
            for id in notes.into_iter().chain([live_id!(results)]) {
                let mut w = self.view.widget(cx, &[id]);
                script_apply_eval!(cx, w, { draw_text +: {color: #(meta)} });
            }
            for i in 0..VERSIONS.len() {
                let mut name = self.view.widget(cx, &[Self::cell(i), live_id!(name)]);
                script_apply_eval!(cx, name, { draw_text +: {color: #(text)} });
                let mut info = self.view.widget(cx, &[Self::cell(i), live_id!(info)]);
                script_apply_eval!(cx, info, { draw_text +: {color: #(meta)} });
                let mut slot = self.view.widget(cx, &[Self::cell(i), live_id!(slot)]);
                script_apply_eval!(cx, slot, { draw_empty +: {color: #(ring)} });
            }
        }
    }

    /// The row's slots: each version's knob once it is compiled, at the
    /// row's size.
    fn fill_slots(&mut self, cx: &mut Cx) {
        // While a compile is timed the row shows rings: every compile runs
        // against the same page, whatever was compiled before it. Drawing
        // the finished knobs would load the frames the compile is polled
        // in, most of all on a software rasteriser.
        let compiling = self.compiling.iter().any(|c| c.is_some());
        let width = self.row_size.max(260.0);
        if self.cell_width != width {
            self.cell_width = width;
            for i in 0..VERSIONS.len() {
                let mut cell = self.view.widget(cx, &[Self::cell(i)]);
                script_apply_eval!(cx, cell, { width: #(width) });
            }
        }
        for i in 0..VERSIONS.len() {
            let knob = if compiling { None } else { self.knobs[i].clone() };
            let slot = self.view.widget(cx, &[Self::cell(i), live_id!(slot)]);
            if let Some(mut s) = slot.borrow_mut::<KnobSlot>() {
                s.size = self.row_size;
                s.set_knob(cx, knob.clone());
            };
            // The sweep's screenshot shows the same knobs.
            let shot = self.view.widget(cx, &[Self::shot(i), live_id!(slot)]);
            if let Some(mut s) = shot.borrow_mut::<KnobSlot>() {
                s.size = self.shot_size;
                s.set_knob(cx, knob);
            };
            self.view.label(cx, &[Self::shot(i), live_id!(name)]).set_text(cx, VERSIONS[i].name);
        }
    }

    /// Rebuilt pages find the knobs of this launch's compiles again. The
    /// shaders are compiled already, so this costs no compile.
    fn restore(&mut self, cx: &mut Cx) {
        self.restored = true;
        self.sized();
        let compiled = cx.global::<KnobCostResults>().sized().compiled.clone();
        for (i, c) in compiled.iter().enumerate() {
            if c.is_some_and(|c| !c.failed) && self.knobs[i].is_none() {
                if let Some(template) = self.template(cx, i) {
                    self.knobs[i] = Some(cx.with_vm(|vm| WidgetRef::script_from_value(vm, template)));
                }
            }
        }
        self.pushed = None;
        self.refresh_texts(cx);
        self.refresh_status(cx);
    }
}

fn compile_line(c: &CompileResult) -> String {
    if c.failed {
        return "did not build (the log says why)".to_string();
    }
    if c.timed_out {
        return format!("no ready after {COMPILE_LIMIT_SECONDS:.0} s");
    }
    if c.warm {
        return "compiled earlier this launch (restart for a cold number)".to_string();
    }
    match c.total_ms {
        Some(ms) => format!("cold compile {ms:.0} ms (codegen {:.0} ms)", c.codegen_ms),
        None => format!("cold compile not measured on Metal (codegen {:.0} ms)", c.codegen_ms),
    }
}

/// "3.21 ms, +2.10 ms, 0.088 ms/knob" against the empty grid.
fn per_knob(v: Option<f64>, base: Option<f64>, knobs: usize, none: &str) -> String {
    match (v, base) {
        (Some(v), Some(b)) => format!("{v:.2} ms/frame, {:+.3} ms/knob", (v - b) / knobs.max(1) as f64),
        (Some(v), None) => format!("{v:.2} ms/frame"),
        _ => none.to_string(),
    }
}

fn columns(v: Option<f64>, base: Option<f64>, knobs: Option<usize>, none: &str) -> (String, String, String) {
    match (v, base, knobs) {
        (Some(v), Some(b), Some(n)) => (
            format!("{v:.2} ms"),
            format!("{:+.2} ms", v - b),
            format!("{:+.3} ms", (v - b) / n.max(1) as f64),
        ),
        (Some(v), _, _) => (format!("{v:.2} ms"), "-".into(), "-".into()),
        _ => (none.to_string(), "-".into(), "-".into()),
    }
}

impl Widget for KnobCost {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.restored {
            self.restore(cx);
        }
        // `MAKEPAD_KNOB_COST_SWEEP=1`: once the page has stayed open a
        // moment, it runs everything by itself (once per process).
        if self.sweep.is_none()
            && self.sweep_armed.is_none()
            && sweep::auto_sweep()
            && !AUTO_SWEEP_STARTED.load(std::sync::atomic::Ordering::Relaxed)
        {
            self.sweep_armed = Some(Cx::monotonic_now() + AUTO_SWEEP_DELAY);
            log!("KNOBCOST ARMED in {AUTO_SWEEP_DELAY} s");
            self.next_frame = cx.new_next_frame();
        }
        self.push(cx);
        self.fill_slots(cx);
        if let Some(mut grid) = self.view.widget(cx, ids!(grid)).borrow_mut::<KnobGrid>() {
            grid.size = self.grid_size;
        }
        self.dpi = cx.current_dpi_factor();
        let m = self.look.material();
        set_material_uniforms(cx, &mut self.view.draw_bg.draw_vars, &m);
        let step = self.view.draw_walk(cx, scope, walk);
        // The pass the page is drawn into; a run labels what it encodes.
        let pass = self.view.draw_bg.area().draw_list_id().and_then(|dl| cx.draw_lists[dl].draw_pass_id);
        if pass.is_some() {
            self.pass = pass;
        }
        if let (Some(pass), Some(run)) = (self.pass, &self.run) {
            let tag = if run.measuring_since.is_some() { run.tag_measure } else { run.tag_warm };
            cx.passes[pass].set_gpu_time_tag(tag);
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.sized();
        if self.next_frame.is_event(event).is_some() {
            self.on_frame(cx);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let measuring = self.run.is_some() || !self.measure_waiting.is_empty() || self.sweep.is_some();
        if self.view.button(cx, ids!(stop)).clicked(&actions) {
            self.stop_sweep(cx);
        }
        if !measuring && self.view.button(cx, ids!(run_all)).clicked(&actions) {
            self.sweep_armed = None;
            self.start_sweep(cx);
        }
        if !measuring {
            for i in 0..VERSIONS.len() {
                if self.view.button(cx, &[Self::cell(i), live_id!(compile)]).clicked(&actions) {
                    self.queue_compiles(cx, &[i]);
                }
                if self.view.button(cx, &[Self::cell(i), live_id!(measure)]).clicked(&actions) {
                    self.queue_measures(cx, &[i]);
                }
            }
            if self.view.button(cx, ids!(compile_all)).clicked(&actions) {
                let all: Vec<usize> = (0..VERSIONS.len()).collect();
                self.queue_compiles(cx, &all);
            }
            if self.view.button(cx, ids!(measure_all)).clicked(&actions) {
                let all: Vec<usize> = (0..VERSIONS.len()).collect();
                self.queue_measures(cx, &all);
            }
        }
        if self.view.button(cx, ids!(copy_results)).clicked(&actions) {
            let table = self.table(cx);
            cx.copy_to_clipboard(&table);
            log!("knob cost: copied\n{table}");
            self.view.label(cx, ids!(status)).set_text(cx, "Copied the table to the clipboard (and to the log).");
        }
        // A knob turned on the page turns them all, and the value control
        // follows.
        let mut turned = None;
        for action in actions.iter() {
            if let Some(wa) = action.as_widget_action() {
                if let TurnedKnobAction::Changed(v) = wa.cast::<TurnedKnobAction>() {
                    turned = Some(v);
                }
            }
        }
        if let Some(v) = turned {
            self.look.value = v;
            self.view.redraw(cx);
            let uid = self.widget_uid();
            cx.widget_action(uid, StoryControlAction::Set { label: VALUE, value: ControlValue::Number(v) });
        }
        if !self.busy() && matches!(event, Event::Actions(_)) && !actions.is_empty() {
            self.refresh_texts(cx);
        }
        cx.extend_actions(actions);
    }
}

// ---- the slot and the grid ----

#[derive(Script, ScriptHook, Widget)]
pub struct KnobSlot {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_empty: DrawQuad,
    #[rust]
    knob: Option<WidgetRef>,
    /// The knob's size, in points; the walk's when 0.
    #[rust]
    size: f64,
    /// Whether the knob takes input here and is listed under this slot. A
    /// second slot showing the same knob (the sweep's screenshot) is not.
    #[live(true)]
    interactive: bool,
    /// The knob's radius as a fraction of the slot, set on the knob before
    /// it draws here (two slots show the same knob at different fills).
    #[live(0.62)]
    fill: f64,
}

impl KnobSlot {
    fn set_knob(&mut self, cx: &mut Cx, knob: Option<WidgetRef>) {
        let same = match (&self.knob, &knob) {
            (Some(a), Some(b)) => a.widget_uid() == b.widget_uid(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some(k) = knob.as_ref().filter(|_| self.interactive) {
                cx.widget_tree_insert_child(self.uid, live_id!(knob), k.clone());
            }
            self.knob = knob;
            self.draw_empty.redraw(cx);
        }
    }
}

impl Widget for KnobSlot {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let walk = if self.size > 0.0 { Walk::fixed(self.size, self.size) } else { walk };
        match &self.knob {
            Some(knob) => {
                if let Some(mut k) = knob.borrow_mut::<TurnedKnob>() {
                    k.fill = self.fill;
                }
                let _ = knob.draw_walk(cx, scope, walk);
            }
            None => {
                self.draw_empty.draw_walk(cx, walk);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(knob) = self.knob.as_ref().filter(|_| self.interactive) {
            knob.handle_event(cx, event, scope);
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct KnobGrid {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_none: DrawQuad,
    /// The knob drawn `count` times, or none: the same space, empty.
    #[rust]
    knob: Option<WidgetRef>,
    #[rust]
    count: usize,
    #[rust]
    size: f64,
    /// The knobs' radius as a fraction of their cell.
    #[live(0.62)]
    fill: f64,
}

impl Widget for KnobGrid {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let size = if self.size > 0.0 { self.size } else { 96.0 };
        cx.begin_turtle(walk, self.layout);
        if let Some(mut k) = self.knob.as_ref().and_then(|k| k.borrow_mut::<TurnedKnob>()) {
            k.fill = self.fill;
        }
        for _ in 0..self.count {
            let cell = Walk::fixed(size, size);
            match &self.knob {
                Some(knob) => {
                    let _ = knob.draw_walk(cx, scope, cell);
                }
                None => {
                    cx.walk_turtle(cell);
                }
            }
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/material/knob-cost",
    category: "Containers",
    component: "Material",
    also: &["TurnedKnob"],
    name: "Knob cost",
    dsl: "MaterialKnobCost",
    added: "2026-09-27",
    tags: &["material", "knob", "cost", "lod", "detail", "compile", "gpu", "benchmark", "bench"],
    doc: "# Knob cost

What each detail level of the knob engine looks like, what it costs per frame and how long its shader takes to compile, measured in Makepad itself: its own shader compiler, its own backend (D3D11 with FXC on Windows, Metal on macOS, OpenGL on Linux) and its own caches.

## The versions

Each version is its own draw shader. It leaves a feature out of the shader's text by replacing the functions that carry it with trivial ones, never by branching around it: Windows compiles with FXC's optimiser off, so a branch nothing takes still costs its full compile.

- **Preview**: no solid at all. A disc shaded from the revolve profile's slope (one lookup, no height taps), a drop shadow, the marks in paint, no wells, grip, cuts or wing.
- **Bare**: the solid's revolve and grip, the wells and the full face shading; no cuts or flat, no wing, no self-shadow, no cast shadow or ground lip, marks in paint.
- **Bare + cuts**: Bare with the cut (dimple, slot, scallops, ring) and the flat back.
- **Bare + wings**: Bare with the wing (ridge or cutters) back; its shadows stay out.
- **Bare + self-shadow**: Bare with the two taps toward the light and the baked self-shadow table back.
- **Bare + cast shadow**: Bare with the revolve's analytic cast-shadow sweep and the ground lip back.
- **Bare + mark finishes**: Bare with engraved, embossed and LED marks and their gradients back.
- **Full**: today's knob, unchanged.

A feature added to Bare comes alone. What features cost together -- a cut notching the cast shadow's foot, the wing's shadow on the ground and on the knob's own top, the contact ring round a notched foot -- only Full has. Every version but Full swaps in a copy of the knob's pixel function with each feature behind a hook; Full keeps the knob's own. `apps/storybook/src/knob/lod.rs` lists which functions each version replaces.

## Compile

**Compile** builds the version's knob widget. That is the moment the shader's text is generated (on the UI thread: the codegen column) and queued for the backend, and the clock starts there. The page then polls once a frame until the backend reports the shader ready (`is_draw_shader_window_ready`): on D3D11 when a worker has run FXC and created the shader, on OpenGL when the driver has compiled and linked the program. The number is a wall-clock latency, rounded up to the frame it was noticed in. **Compile all** compiles one version after another, so none shares the machine with another, and while a compile is timed the row shows rings instead of the knobs already built, so every compile runs against the same page. Where the driver compiles synchronously (Mesa's llvmpipe does), the compile blocks the frame after Compile; where it compiles on its own threads, the page keeps drawing meanwhile.

Every launch salts the versions' shader text with a number drawn from the clock, in a branch no pixel takes. The DXBC cache (`%LOCALAPPDATA%\\makepad\\d3d11_shader_cache`), the GL program cache and the driver's own caches are all keyed on the text, so none of them has seen it: the compile is cold. Each compile leaves one new entry in those caches, which the next launch never reads. A version compiled once is compiled for the rest of the launch; restart for a new cold number. The table's text column is the generated shader's size. On Metal the backend does not report readiness, so the compile is not measured there.

## Frame cost

**Measure** draws a grid of 24 knobs of the version (96 pt; the Controls tab changes the size) and redraws the page every frame for about two seconds after half a second of warm-up; the same grid empty is measured first and, when more than one version is measured, again at the end, and the knobs are compared with the mean of the two. A version not compiled yet is compiled first. For each run the page reports the medians of:

- **GPU**: the GPU time of the pass the page draws into, from the platform's per-pass GPU timer, tagged per run so a late sample is never counted in the wrong one. Metal and D3D11 record it; on OpenGL it reads *not recorded on this backend*.
- **CPU**: the interval between frames.

The difference from the empty grid, per frame and per knob, is the knobs' cost. The page is idle otherwise: nothing redraws until a button is pressed.

## What distorts the numbers

- **Vsync**: where the display syncs, the frame interval rounds up to its refresh, so the CPU column shows a knob's cost only once a frame no longer fits. The GPU column is not bound by it.
- **Everything else on the machine**: other apps, the storybook's own panels, a browser. The empty grid is measured in the same conditions, before and after, to take what it can out; the first line of the table shows both runs, so a drift between them shows.
- **First draws**: a pipeline's first use, the knob's bakes and texture uploads land in the first frames, which the warm-up throws away. A driver that finishes a shader's compile only at its first draw moves that cost out of the compile number into the first frame.
- **The caches, on purpose**: a real launch after the first reads the DXBC or program cache and compiles far faster. The page bypasses them so the number is the worst case, the first launch after an update.
- **Software rendering**: on a software rasteriser the GPU work runs on the CPU and shows up in the frame interval instead.

## Run everything

**Run everything** does all of it unattended: every version compiled once, cold, then six materials (Neumorphic dark, Neumorphic, Glossy, Milled, Chrome, Porcelain) by six styles (Classic, Knurled, Winged, Scalloped, Cutwing, Skirted). For each look it shows all eight versions side by side and holds still for 1.2 s -- a window for a screenshot, announced in the log as `KNOBCOST READY step=i/n material=... style=... rects=...` with each knob's window-local rectangle -- then measures every version on that look (1.5 s each, the empty grid before and after). The page says where it is and **Stop** ends it early. At the end the results go to `local/knob-cost/results-<unix time>.json` under the working directory when it has a `local/` directory, else the system's temp directory, and to the clipboard; the log says `KNOBCOST DONE path=...`. `MAKEPAD_KNOB_COST_SWEEP=1` starts it by itself once the page has been open a moment. On a software rasteriser it takes about a quarter of an hour.

## Copying the results

**Copy results** puts the table on the clipboard and in the log, ready to paste: the backend, the adapter where the platform names it, the look and the grid, each version's cold compile, codegen time, shader size, and GPU and CPU per frame and per knob.",
    subject: "",
    feature: None,
    controls: crate::knob_controls!(
        section("Knob cost", true),
        number("Row knob size (pt)", "row_size", 48., 280., 2., 200.),
        number("Grid knob size (pt)", "grid_size", 24., 160., 4., 96.),
    ),
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every detail level's knob builds from the page's template and its
    /// draw shader compiles. The versions carry copies of the knob's own
    /// functions and call the rest by name, so a shared function changed
    /// under them fails here, where the compiler never looks.
    #[test]
    fn every_version_builds_and_its_shader_compiles() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            super::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "the page's own shaders failed to compile");
        for (i, version) in VERSIONS.iter().enumerate() {
            let template = page
                .borrow::<KnobCost>()
                .expect("the page is a KnobCost")
                .template(&mut cx, i)
                .unwrap_or_else(|| panic!("no template for {}", version.key));
            let knob = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template));
            assert!(knob.borrow::<TurnedKnob>().is_some(), "{} is no knob", version.name);
            assert_eq!(makepad_platform::shader_error::take(), None, "{}'s shader failed to compile", version.name);
        }
    }
}
