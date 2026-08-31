//! Manual harness for the DirectComposition test plan.
//!
//! ```text
//! cargo run -p makepad-example-direct-composition
//! ```
//!
//! The launcher is a normal HWND window. The lab and twin windows opt into
//! DirectComposition at create time (`window.direct_composition` cannot be
//! changed after the HWND exists). Click a test on the launcher; the lab
//! window applies that scene.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const BACKDROP_EXTENT: f32 = 16384.0;
const VIDEO_CYAN: Vec4 = vec4(0.0, 0.85, 1.0, 1.0);
const VIDEO_MAGENTA: Vec4 = vec4(1.0, 0.2, 0.75, 1.0);
const TWIN_FILL: Vec4 = vec4(1.0, 0.45, 0.15, 1.0);
const HOLE_OPAQUE: Vec4 = vec4(0.05, 0.07, 0.10, 1.0);
const HOLE_PUNCH: Vec4 = vec4(0.0, 0.0, 0.0, 0.0);
const PASS_OPAQUE: Vec4 = vec4(0.05, 0.07, 0.10, 1.0);
const PASS_PUNCH: Vec4 = vec4(0.0, 0.0, 0.0, 0.0);
const PASS_CLEAR: Vec4 = vec4(0.12, 0.28, 0.55, 0.35);

script_mod! {
    use mod.prelude.widgets.*

    let Card = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 6
        padding: 12
        new_batch: true
        draw_bg.color: #x151c27
        draw_bg.border_radius: 12.0
        kicker := Label{
            text: "T00"
            draw_text.color: #x8fc7ff
            draw_text.text_style.font_size: 11
        }
        title := Label{
            text: "Title"
            draw_text.color: #xeff5ff
            draw_text.text_style: theme.font_bold{font_size: 14}
        }
        expect := Label{
            width: Fill
            text: "Expected"
            draw_text.color: #x97a9c0
            draw_text.text_style.font_size: 11
        }
        run := Button{
            width: Fit
            height: Fit
            text: "Run"
        }
    }

    let CliCard = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 4
        padding: 12
        new_batch: true
        draw_bg.color: #x121820
        draw_bg.border_radius: 12.0
        kicker := Label{
            text: "CLI"
            draw_text.color: #xffd280
            draw_text.text_style.font_size: 11
        }
        title := Label{
            text: "Title"
            draw_text.color: #xeff5ff
            draw_text.text_style: theme.font_bold{font_size: 13}
        }
        cmd := Label{
            width: Fill
            text: "cargo test"
            draw_text.color: #x9fd3af
            draw_text.text_style.font_size: 11
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "DComp tests — launcher (HWND)"
                window.inner_size: vec2(520, 820)
                window.position: vec2(40, 40)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 10
                        padding: 16
                        draw_bg.color: #x0d1117

                        Label{
                            text: "DirectComposition test harness"
                            draw_text.color: #xeff5ff
                            draw_text.text_style: theme.font_bold{font_size: 22}
                        }
                        Label{
                            width: Fill
                            text: "This window is the default HWND path. The lab and twin windows on the right opt into DirectComposition at create time. Click Run; watch the lab."
                            draw_text.color: #x97a9c0
                            draw_text.text_style.font_size: 12
                        }
                        launcher_status := Label{
                            width: Fill
                            text: "Ready."
                            draw_text.color: #x9fd3af
                            draw_text.text_style.font_size: 12
                        }

                        ScrollYView{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 8

                            t01 := Card{
                                kicker.text: "T01 · HWND"
                                title.text: "Default window"
                                expect.text: "This launcher. Opaque UI, resize leaves no composition stretch gap (HWND uses SCALING_NONE + background colour)."
                                run.text: "Highlight"
                            }
                            t02 := Card{
                                kicker.text: "T02 · create-time"
                                title.text: "Composition, opaque widgets"
                                expect.text: "Lab is not blank. Opaque chrome. Resize stretches the UI swap chain rather than showing a background-colour gap."
                            }
                            t03 := Card{
                                kicker.text: "T03 · alpha"
                                title.text: "Pass clear alpha < 1, no cover"
                                expect.text: "Lab chrome hides. Semi-transparent blue clear. Desktop (or backdrop) shows through. Opaque widgets, if any, keep premultiplied edges."
                            }
                            t04 := Card{
                                kicker.text: "T04 · alpha"
                                title.text: "Opaque pass.clear_color"
                                expect.text: "Cyan behind-UI child is created but hidden: opaque clear covers the buffer. Run T09 to punch a hole and reveal it."
                            }
                            t05 := Card{
                                kicker.text: "T05 · runtime"
                                title.text: "set_transparent after create"
                                expect.text: "Calls set_transparent(true) on the lab. Composition windows ignore it — the window must not go blank."
                            }
                            t06 := Card{
                                kicker.text: "T06 · create-time"
                                title.text: "Mica + composition"
                                expect.text: "Lab is created with window.backdrop: Mica. Composition logs that DWM backdrops are ignored. Window still draws."
                            }
                            t07 := Card{
                                kicker.text: "T07 · popup"
                                title.text: "DropDown on a composition window"
                                expect.text: "Open the DropDown on the lab chrome. The menu is still CreateSwapChainForHwnd (a popup) and must stay visible."
                            }
                            t08 := Card{
                                kicker.text: "T08 · fallback"
                                title.text: "Device probe / is_direct_composition"
                                expect.text: "Status line reports composition_active. If DCompositionCreateDevice failed, lab is a normal HWND, not a blank NOREDIRECTIONBITMAP window."
                            }
                            t09 := Card{
                                kicker.text: "T09 · child"
                                title.text: "BEHIND child + hole"
                                expect.text: "Cyan fill in the hole, chrome around it. Clicks hit Makepad (try the DropDown) — no overlay HWND."
                            }
                            t10 := Card{
                                kicker.text: "T10 · child"
                                title.text: "BACKDROP then BEHIND"
                                expect.text: "Black fill under everything (created first). Cyan BEHIND created after still sits on top of it. A hole larger than the cyan child shows black, not the desktop."
                            }
                            t11a := Card{
                                kicker.text: "T11a · resize"
                                title.text: "Drag-resize without BACKDROP"
                                expect.text: "Cyan behind-UI child, punched hole, no fill. Drag the lab edges: hole/child disagreement flashes the desktop."
                            }
                            t11b := Card{
                                kicker.text: "T11b · resize"
                                title.text: "Drag-resize with BACKDROP"
                                expect.text: "Same as T11a plus the black fill. Drag again: flashes stay the fill colour."
                            }
                            t12 := Card{
                                kicker.text: "T12 · child"
                                title.text: "Hide (visible: false)"
                                expect.text: "Cyan child leaves the tree (hole shows backdrop or desktop). Run T09 to show it again."
                            }
                            t13 := Card{
                                kicker.text: "T13 · child"
                                title.text: "set_child_z FRONT / BEHIND"
                                expect.text: "First click: cyan promoted to FRONT, covering the hole label. Second click (run T09): back behind the UI."
                            }
                            t14a := Card{
                                kicker.text: "T14a · solid"
                                title.text: "Same solid colour every frame"
                                expect.text: "Calls dcomp_set_child_solid(cyan) at 60 Hz. Host drops duplicates — no per-frame swap chain. Colour stays cyan."
                            }
                            t14b := Card{
                                kicker.text: "T14b · solid"
                                title.text: "Change solid colour"
                                expect.text: "One dcomp_set_child_solid(magenta). Child repaints. Then T14a can repeat magenta without rebuilding."
                            }
                            t15 := Card{
                                kicker.text: "T15 · windows"
                                title.text: "Two composition windows"
                                expect.text: "Lab + the smaller twin (orange fill). Both trees publish from one Commit. Close the twin: lab must not go blank."
                            }
                            t16 := Card{
                                kicker.text: "T16 · child"
                                title.text: "dcomp_remove_child"
                                expect.text: "Removes the cyan child. Hole empty (backdrop or desktop). Lab window stays up — no use-after-free / blank UI."
                            }

                            CliCard{
                                kicker.text: "T17 · cargo"
                                title.text: "window::tests"
                                cmd.text: "cargo test -p makepad-platform --lib window::tests"
                            }
                            CliCard{
                                kicker.text: "T18 · cargo"
                                title.text: "dcomp::tests"
                                cmd.text: "cargo test -p makepad-platform --lib dcomp::tests"
                            }
                            CliCard{
                                kicker.text: "T19 · cargo"
                                title.text: "non-Windows cfg"
                                cmd.text: "cargo check -p makepad-platform --target aarch64-linux-android"
                            }
                            CliCard{
                                kicker.text: "T20 · cargo"
                                title.text: "windows-strip after new DComp symbols"
                                cmd.text: "cargo run --release --manifest-path tools/windows_strip/Cargo.toml"
                            }
                        }
                    }
                }
            }

            lab_window := Window{
                window.title: "DComp tests — lab (composition)"
                window.inner_size: vec2(720, 640)
                window.position: vec2(580, 40)
                window.direct_composition: true
                window.backdrop: mod.draw.WindowBackdrop.Mica
                pass.clear_color: #0000
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    show_bg: false

                    lab_status := SolidView{
                        width: Fill
                        height: Fit
                        padding: 10
                        new_batch: true
                        draw_bg.color: #x151c27
                        lab_status_text := Label{
                            width: Fill
                            text: "Lab ready."
                            draw_text.color: #x9fd3af
                            draw_text.text_style.font_size: 12
                        }
                    }

                    lab_chrome := SolidView{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        padding: 10
                        align: Align{x: 0.0, y: 0.5}
                        new_batch: true
                        draw_bg.color: #x1c283a
                        Label{
                            text: "Popup:"
                            draw_text.color: #xc9d5e7
                        }
                        popup_probe := DropDown{
                            width: 160
                            labels: ["Popup A" "Popup B" "Popup C"]
                        }
                        Label{
                            text: "← T07"
                            draw_text.color: #x8fc7ff
                            draw_text.text_style.font_size: 11
                        }
                    }

                    lab_stage := View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        lab_hole := SolidView{
                            width: Fill
                            height: Fill
                            draw_bg.color: #000
                        }

                        lab_overlay := View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5, y: 0.5}
                            lab_overlay_label := Label{
                                text: "UI overlay (should sit above BEHIND)"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 16}
                            }
                        }
                    }
                }
            }

            twin_window := Window{
                window.title: "DComp tests — twin (composition)"
                window.inner_size: vec2(420, 220)
                window.position: vec2(580, 700)
                window.direct_composition: true
                pass.clear_color: #0000
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    show_bg: false
                    twin_bar := SolidView{
                        width: Fill
                        height: Fit
                        padding: 10
                        new_batch: true
                        draw_bg.color: #x151c27
                        twin_status := Label{
                            width: Fill
                            text: "Twin. Close me during T15; the lab must stay up."
                            draw_text.color: #xffc86f
                            draw_text.text_style.font_size: 12
                        }
                    }
                    twin_hole := SolidView{
                        width: Fill
                        height: Fill
                        draw_bg.color: #0000
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestId {
    T01,
    T02,
    T03,
    T04,
    T05,
    T06,
    T07,
    T08,
    T09,
    T10,
    T11a,
    T11b,
    T12,
    T13,
    T14a,
    T14b,
    T15,
    T16,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(1.0)]
    dpi: f64,
    #[rust(Timer::empty())]
    solid_timer: Timer,
    #[rust]
    current: Option<TestId>,
    #[cfg(target_os = "windows")]
    #[rust]
    lab_backdrop: Option<DcompChildId>,
    #[cfg(target_os = "windows")]
    #[rust]
    lab_video: Option<DcompChildId>,
    #[cfg(target_os = "windows")]
    #[rust]
    twin_fill: Option<DcompChildId>,
    #[cfg(target_os = "windows")]
    #[rust(VIDEO_CYAN)]
    video_color: Vec4,
    #[cfg(target_os = "windows")]
    #[rust]
    video_front: bool,
    #[cfg(target_os = "windows")]
    #[rust]
    repeat_solid: bool,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.refresh_status(cx, "Ready. Lab and twin are DirectComposition windows.");
        self.apply_test(cx, TestId::T02);
        self.ensure_twin_fill(cx);
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.solid_timer.is_timer(event).is_some() {
            self.tick_solid_repeat(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(label) = self.ui.drop_down(cx, ids!(popup_probe)).changed_label(actions) {
            self.set_lab_status(cx, &format!("DropDown selected {label:?} — popup HWND still visible?"));
        }

        let clicks = [
            (ids!(t01.run), TestId::T01),
            (ids!(t02.run), TestId::T02),
            (ids!(t03.run), TestId::T03),
            (ids!(t04.run), TestId::T04),
            (ids!(t05.run), TestId::T05),
            (ids!(t06.run), TestId::T06),
            (ids!(t07.run), TestId::T07),
            (ids!(t08.run), TestId::T08),
            (ids!(t09.run), TestId::T09),
            (ids!(t10.run), TestId::T10),
            (ids!(t11a.run), TestId::T11a),
            (ids!(t11b.run), TestId::T11b),
            (ids!(t12.run), TestId::T12),
            (ids!(t13.run), TestId::T13),
            (ids!(t14a.run), TestId::T14a),
            (ids!(t14b.run), TestId::T14b),
            (ids!(t15.run), TestId::T15),
            (ids!(t16.run), TestId::T16),
        ];
        for (id, test) in clicks {
            if self.ui.button(cx, id).clicked(actions) {
                self.apply_test(cx, test);
                break;
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowGeomChange(change) = event {
            let lab = self.ui.window(cx, ids!(lab_window));
            if lab.window_id() == Some(change.window_id) {
                self.dpi = change.new_geom.dpi_factor;
            }
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if matches!(event, Event::Draw(_) | Event::WindowGeomChange(_)) {
            self.sync_child_geom(cx);
        }
    }
}

impl App {
    fn apply_test(&mut self, cx: &mut Cx, test: TestId) {
        self.current = Some(test);
        self.stop_solid_repeat(cx);

        match test {
            TestId::T01 => {
                self.refresh_status(
                    cx,
                    "T01: this launcher is the HWND path. composition should be false here.",
                );
            }
            TestId::T02 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_OPAQUE);
                self.set_hole_color(cx, HOLE_OPAQUE);
                self.teardown_lab_children(cx);
                self.refresh_status(
                    cx,
                    "T02: opaque composition UI. Resize the lab — it should stretch, not gap.",
                );
            }
            TestId::T03 => {
                self.set_lab_cover(cx, false);
                self.set_pass_clear(cx, PASS_CLEAR);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.teardown_lab_children(cx);
                self.refresh_status(
                    cx,
                    "T03: chrome hidden, semi-transparent clear. Desktop should show through.",
                );
            }
            TestId::T04 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_OPAQUE);
                self.set_hole_color(cx, HOLE_OPAQUE);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.refresh_status(
                    cx,
                    "T04: opaque clear + opaque hole. Cyan child exists but must stay hidden.",
                );
            }
            TestId::T05 => {
                self.ui.window(cx, ids!(lab_window)).set_transparent(cx, true);
                self.refresh_status(
                    cx,
                    "T05: set_transparent(true) on the lab. Composition must ignore it and not blank.",
                );
            }
            TestId::T06 => {
                self.ui
                    .window(cx, ids!(lab_window))
                    .set_backdrop(cx, WindowBackdrop::Mica);
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_OPAQUE);
                self.set_hole_color(cx, HOLE_OPAQUE);
                self.refresh_status(
                    cx,
                    "T06: lab created with Mica; runtime set_backdrop(Mica) too. Window must still draw.",
                );
            }
            TestId::T07 => {
                self.set_lab_cover(cx, true);
                self.refresh_status(
                    cx,
                    "T07: open the DropDown on the lab chrome. The popup is an HWND swap chain.",
                );
            }
            TestId::T08 => {
                self.refresh_status(cx, "T08: read composition flags below. Fallback = HWND, not blank.");
            }
            TestId::T09 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.teardown_lab_children(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.refresh_status(
                    cx,
                    "T09: cyan BEHIND in the hole, UI overlay on top. Clicks still hit Makepad.",
                );
            }
            TestId::T10 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.teardown_lab_children(cx);
                self.ensure_backdrop(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.refresh_status(
                    cx,
                    "T10: black BACKDROP first, then cyan BEHIND. Overshoot lands on black.",
                );
            }
            TestId::T11a => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.teardown_lab_children(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.refresh_status(
                    cx,
                    "T11a: no BACKDROP. Drag-resize the lab — edges should flash the desktop.",
                );
            }
            TestId::T11b => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.teardown_lab_children(cx);
                self.ensure_backdrop(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.refresh_status(
                    cx,
                    "T11b: BACKDROP on. Drag-resize — flashes should stay black, not the desktop.",
                );
            }
            TestId::T12 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_backdrop(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.set_video_visible(cx, false);
                self.refresh_status(
                    cx,
                    "T12: cyan geom.visible = false. Hole shows the black fill. Run T09 to restore.",
                );
            }
            TestId::T13 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_video(cx, VIDEO_CYAN, true);
                self.refresh_status(
                    cx,
                    "T13: cyan z = FRONT. It should cover the hole label. Run T09 to put it behind.",
                );
            }
            TestId::T14a => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.start_solid_repeat(cx);
                self.refresh_status(
                    cx,
                    "T14a: set_child_solid(same colour) every frame. Host should drop duplicates.",
                );
            }
            TestId::T14b => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_video(cx, VIDEO_MAGENTA, false);
                self.refresh_status(
                    cx,
                    "T14b: solid colour changed to magenta. Then T14a repeats without rebuilding.",
                );
            }
            TestId::T15 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_backdrop(cx);
                self.ensure_video(cx, VIDEO_CYAN, false);
                self.ensure_twin_fill(cx);
                self.refresh_status(
                    cx,
                    "T15: lab + orange twin. Close the twin; this lab must not go blank.",
                );
            }
            TestId::T16 => {
                self.set_lab_cover(cx, true);
                self.set_pass_clear(cx, PASS_PUNCH);
                self.set_hole_color(cx, HOLE_PUNCH);
                self.ensure_backdrop(cx);
                self.remove_video(cx);
                self.refresh_status(
                    cx,
                    "T16: cyan child removed. Lab UI stays. Hole shows the black fill.",
                );
            }
        }
    }

    fn refresh_status(&mut self, cx: &mut Cx, message: &str) {
        let launcher = self.ui.window(cx, ids!(main_window));
        let lab = self.ui.window(cx, ids!(lab_window));
        let twin = self.ui.window(cx, ids!(twin_window));
        let line = format!(
            "{message}\nlauncher composition={} · lab composition={} · twin composition={}",
            launcher.is_direct_composition(cx),
            lab.is_direct_composition(cx),
            twin.is_direct_composition(cx),
        );
        self.ui.label(cx, ids!(launcher_status)).set_text(cx, &line);
        self.set_lab_status(cx, &line);
    }

    fn set_lab_status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(lab_status_text)).set_text(cx, text);
    }

    fn set_lab_cover(&mut self, cx: &mut Cx, show: bool) {
        self.ui.view(cx, ids!(lab_chrome)).set_visible(cx, show);
        self.ui.view(cx, ids!(lab_status)).set_visible(cx, true);
        self.ui.view(cx, ids!(lab_overlay)).set_visible(cx, show);
        self.ui.view(cx, ids!(lab_hole)).set_visible(cx, true);
        self.ui.view(cx, ids!(lab_stage)).redraw(cx);
    }

    fn set_hole_color(&mut self, cx: &mut Cx, color: Vec4) {
        let mut hole = self.ui.view(cx, ids!(lab_hole));
        script_apply_eval!(cx, hole, {
            draw_bg +: { color: #(color) }
        });
        hole.redraw(cx);
    }

    fn set_pass_clear(&mut self, cx: &mut Cx, color: Vec4) {
        let mut lab = self.ui.window(cx, ids!(lab_window));
        script_apply_eval!(cx, lab, {
            pass.clear_color: #(color)
        });
        lab.redraw(cx);
    }

    fn start_solid_repeat(&mut self, cx: &mut Cx) {
        self.stop_solid_repeat(cx);
        #[cfg(target_os = "windows")]
        {
            self.repeat_solid = true;
        }
        self.solid_timer = cx.start_interval(1.0 / 60.0);
    }

    fn stop_solid_repeat(&mut self, cx: &mut Cx) {
        #[cfg(target_os = "windows")]
        {
            self.repeat_solid = false;
        }
        if self.solid_timer.is_empty() {
            return;
        }
        cx.stop_timer(self.solid_timer);
        self.solid_timer = Timer::empty();
    }

    fn tick_solid_repeat(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            if !self.repeat_solid {
                return;
            }
            let Some(video) = self.lab_video else {
                return;
            };
            self.ui
                .window(cx, ids!(lab_window))
                .dcomp_set_child_solid(cx, video, self.video_color);
        }
    }

    fn teardown_lab_children(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            let window = self.ui.window(cx, ids!(lab_window));
            if let Some(id) = self.lab_video.take() {
                window.dcomp_remove_child(cx, id);
            }
            if let Some(id) = self.lab_backdrop.take() {
                window.dcomp_remove_child(cx, id);
            }
            self.video_front = false;
        }
    }

    fn ensure_backdrop(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            if self.lab_backdrop.is_some() {
                return;
            }
            let window = self.ui.window(cx, ids!(lab_window));
            let Some(id) = window.dcomp_create_child(cx, DcompChildZ::BACKDROP) else {
                return;
            };
            window.dcomp_set_child_solid(cx, id, vec4(0.0, 0.0, 0.0, 1.0));
            window.dcomp_set_child_geom(
                cx,
                id,
                DcompChildGeom {
                    x: 0.0,
                    y: 0.0,
                    width: BACKDROP_EXTENT,
                    height: BACKDROP_EXTENT,
                    scale_x: BACKDROP_EXTENT,
                    scale_y: BACKDROP_EXTENT,
                    visible: true,
                },
            );
            self.lab_backdrop = Some(id);
        }
    }

    fn ensure_video(&mut self, cx: &mut Cx, color: Vec4, front: bool) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (cx, color, front);
        }
        #[cfg(target_os = "windows")]
        {
            let window = self.ui.window(cx, ids!(lab_window));
            let z = if front {
                DcompChildZ::FRONT
            } else {
                DcompChildZ::BEHIND
            };
            if self.lab_video.is_none() {
                self.lab_video = window.dcomp_create_child(cx, z);
            }
            let Some(id) = self.lab_video else {
                return;
            };
            if self.video_front != front {
                window.dcomp_set_child_z(cx, id, z);
            }
            window.dcomp_set_child_solid(cx, id, color);
            self.video_color = color;
            self.video_front = front;
            self.sync_child_geom(cx);
        }
    }

    fn set_video_visible(&mut self, cx: &mut Cx, visible: bool) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (cx, visible);
        }
        #[cfg(target_os = "windows")]
        {
            let Some(id) = self.lab_video else {
                return;
            };
            let mut geom = self.video_geom(cx);
            geom.visible = visible;
            self.ui
                .window(cx, ids!(lab_window))
                .dcomp_set_child_geom(cx, id, geom);
        }
    }

    fn remove_video(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(id) = self.lab_video.take() else {
                return;
            };
            self.ui
                .window(cx, ids!(lab_window))
                .dcomp_remove_child(cx, id);
            self.video_front = false;
        }
    }

    fn ensure_twin_fill(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            if self.twin_fill.is_some() {
                return;
            }
            let window = self.ui.window(cx, ids!(twin_window));
            let Some(id) = window.dcomp_create_child(cx, DcompChildZ::BACKDROP) else {
                return;
            };
            window.dcomp_set_child_solid(cx, id, TWIN_FILL);
            window.dcomp_set_child_geom(
                cx,
                id,
                DcompChildGeom {
                    x: 0.0,
                    y: 0.0,
                    width: BACKDROP_EXTENT,
                    height: BACKDROP_EXTENT,
                    scale_x: BACKDROP_EXTENT,
                    scale_y: BACKDROP_EXTENT,
                    visible: true,
                },
            );
            self.twin_fill = Some(id);
        }
    }

    fn sync_child_geom(&mut self, cx: &mut Cx) {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(id) = self.lab_video else {
                return;
            };
            let geom = self.video_geom(cx);
            if !geom.is_shown() {
                return;
            }
            self.ui
                .window(cx, ids!(lab_window))
                .dcomp_set_child_geom(cx, id, geom);
        }
    }

    #[cfg(target_os = "windows")]
    fn video_geom(&self, cx: &Cx) -> DcompChildGeom {
        let rect = self.ui.view(cx, ids!(lab_hole)).area().rect(cx);
        let dpi = if self.dpi > 0.0 { self.dpi } else { 1.0 };
        let width = (rect.size.x * dpi).max(0.0) as f32;
        let height = (rect.size.y * dpi).max(0.0) as f32;
        DcompChildGeom {
            x: (rect.pos.x * dpi) as f32,
            y: (rect.pos.y * dpi) as f32,
            width,
            height,
            scale_x: width.max(1.0),
            scale_y: height.max(1.0),
            visible: true,
        }
    }
}
